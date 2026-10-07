use mcp_server_middleware::*;
use rust_extensions::AsStr;
use serde::{Deserialize, Serialize};

use crate::board::{
    BoardInner, GoalModel, ProjectModel, ReleaseModel, TaskModel, UserModel, compose_task_handle,
};

/// One column of a board, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ColumnView {
    #[property(
        description = "The column id — this is what a task's `status` is set to. `todo` and `done` exist in every project and are always the first and last column"
    )]
    pub id: String,
    #[property(description = "What the column is called on the board")]
    pub name: String,
    #[property(description = "What belongs in this column, as the person who created it wrote it")]
    pub description: String,
}

/// One kind of work, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct KindView {
    #[property(description = "The kind id — this is what a task's `kind` is set to")]
    pub id: String,
    #[property(description = "What the kind is called on the board")]
    pub name: String,
    #[property(
        description = "What qualifies as this kind, as the person who created it wrote it. Read it before classifying a task — the meanings are per project and are not the ones you would guess"
    )]
    pub description: String,
    #[property(description = "The colour the board draws this kind in")]
    pub color: String,
}

/// One project, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ProjectView {
    #[property(
        description = "The project's prefix, e.g. `RMS`. This is how a project is named in every other tool, and the first half of every task id on it"
    )]
    pub prefix: String,
    #[property(description = "What the project is called")]
    pub name: String,
    #[property(description = "What the project is about, as the person who set it up wrote it")]
    pub description: String,
    #[property(
        description = "The board's columns in order, `todo` first and `done` last. A task's `status` must be one of these ids"
    )]
    pub columns: Vec<ColumnView>,
    #[property(
        description = "The kinds of work configured for this project. May be empty — a kind is optional on a task"
    )]
    pub kinds: Vec<KindView>,
    #[property(
        description = "Every label currently in use on this project's tasks, sorted. Not a fixed list: a label exists exactly as long as a task wears it, and a new one is created simply by putting it on a task"
    )]
    pub labels: Vec<String>,
    #[property(
        description = "Emails of the people who may see this board. This is the list to resolve a spoken name against before putting it in `assignee`"
    )]
    pub members: Vec<String>,
    #[property(description = "How many tasks are on the board, in every column")]
    pub tasks_amount: i32,
    #[property(
        description = "The goals of this project that are still open, oldest first. Work here is organised BY GOAL: a goal is the container a conversation happens at and tasks come out of, and this is where you find the ids to pass as `goal`. Closed goals are left out — goals_list with include_archived brings the history back"
    )]
    pub goals: Vec<GoalView>,
}

impl ProjectView {
    pub fn from_model(project: &ProjectModel, board: &BoardInner) -> Self {
        let mut columns = Vec::with_capacity(project.columns.len() + 2);

        columns.push(ColumnView {
            id: task_manager_shared::projects::COLUMN_ID_TODO.to_string(),
            name: "Todo".to_string(),
            description: "Not started. Also where a task with an unrecognised status shows up"
                .to_string(),
        });

        for column in &project.columns {
            columns.push(ColumnView {
                id: column.id.clone(),
                name: column.name.clone(),
                description: column.description.clone(),
            });
        }

        columns.push(ColumnView {
            id: task_manager_shared::projects::COLUMN_ID_DONE.to_string(),
            name: "Done".to_string(),
            description: "Landed. A dependency counts as satisfied only when it is here"
                .to_string(),
        });

        Self {
            prefix: project.prefix.clone(),
            name: project.name.clone(),
            description: project.description.clone(),
            columns,
            kinds: project
                .kinds
                .iter()
                .map(|kind| KindView {
                    id: kind.id.clone(),
                    name: kind.name.clone(),
                    description: kind.description.clone(),
                    color: kind.color.as_str().to_string(),
                })
                .collect(),
            labels: board.labels_of_project(&project.id),
            members: project.members.iter().cloned().collect(),
            tasks_amount: board.tasks_amount(&project.id) as i32,
            // Open ones only. A project that has run for a year would otherwise answer the very first
            // call with every epic it has ever finished, and the one thing this list is for is telling a
            // caller where to put the work it is about to create.
            goals: board
                .goals_of_project(&project.id)
                .iter()
                .filter(|goal| !goal.is_closed())
                .map(|goal| GoalView::from_model(goal, project, board))
                .collect(),
        }
    }
}

/// One goal, as a tool sees it.
///
/// The container work is done around, and the level a conversation happens at: a goal is discussed, tasks
/// come out of the discussion, and the resolution goes back onto it when it lands.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GoalView {
    #[property(
        description = "The goal id, like `RMS-G7`. This is how a goal is named everywhere — in `goal` on a task, in goals_update, in tasks_list. The number comes from the same counter task numbers come from, so `RMS-7` and `RMS-G7` are never both real; the `G` is what says which kind you are holding"
    )]
    pub id: String,
    #[property(description = "The prefix of the project this goal is on")]
    pub project: String,
    #[property(description = "What the goal is called")]
    pub name: String,
    #[property(description = "What it is about, as Markdown")]
    pub description: String,
    #[property(
        description = "The palette colour the board marks this goal's work with. Visual only — but worth reading before colouring a new goal, so two goals on one board are not the same colour"
    )]
    pub color: String,
    #[property(
        description = "How urgent the goal is: `super-high`, `high`, `normal`, `low` or `super-low`. The same scale a task carries, and what puts the goal where it is on the Goals screen — most urgent first. Not derived from its tasks: an epic can be urgent while none of its work has started"
    )]
    pub priority: String,
    #[property(
        description = "`todo` while the goal is open, `done` once it is closed. Derived from whether it has been closed, so it cannot disagree with `closed_unix_seconds` — a goal has these two states and nothing in between in this version. The Goals screen shows a third, `In Progress`, which is NOT this field: it is derived in the browser from whether any task under the goal has moved off `todo`, so read `done_amount` and the tasks themselves rather than this to tell a started goal from an untouched one"
    )]
    pub status: String,
    #[property(
        description = "How many tasks are part of this goal, INCLUDING work already archived off the board. Do not recompute this from tasks_list: that leaves archived work out, and an old goal would read as half-done"
    )]
    pub tasks_amount: i32,
    #[property(description = "How many of those tasks are done. The goal is closable when it equals `tasks_amount`")]
    pub done_amount: i32,
    #[property(
        description = "The goal's own checklist, in the order it was written — the notes-to-self of the epic, kept inside it. Separate from `tasks_amount` / `done_amount`, which count its TASKS and are what decide whether it can close: an unticked item here does not hold the goal open. Use it for the small things an epic drags along that are not worth a card of their own. Usually empty"
    )]
    pub subtasks: Vec<SubtaskView>,
    #[property(
        description = "References to the documents this goal points at, each a url: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. Read one by handing it to documents_get, which takes a reference wherever it takes an id. A goal is where a decision gets written down, so this is where the specification behind an epic is usually attached. A reference here may have gone stale: documents are deleted by moving them to the trash and a repository's file can go upstream, and a reference is never quietly dropped, because restoring one is a call away"
    )]
    pub documents: Vec<String>,
    #[property(
        description = "The releases this goal went out in, newest first — each one whole, with its services and their versions. A goal is the description of a feature and a release is the record of it shipping, so THIS is what answers 'is it out, and in which version of what'. Empty means nothing under this goal has been recorded as released, which is not the same as nothing being deployed. Attach one with `goal` on releases_create, or with add_releases on goals_update"
    )]
    pub releases: Vec<ReleaseView>,
    #[property(
        description = "How many notes are on the goal's thread. This is where the reasoning lives — read it with goals_get_comments before acting on a goal somebody else shaped"
    )]
    pub comments_amount: i32,
    #[property(description = "When the goal was created, unix seconds (UTC)")]
    pub created_unix_seconds: i64,
    #[property(description = "When the goal itself last changed, unix seconds (UTC). A comment does not move this")]
    pub updated_unix_seconds: i64,
    #[property(
        description = "When it was closed, unix seconds (UTC), and absent while it is open. A goal closed longer ago than the project's archive window is left out of goals_list unless you ask for it"
    )]
    pub closed_unix_seconds: Option<i64>,
    #[property(
        description = "When it was DELETED, unix seconds (UTC), and absent for a goal that is not. A deleted goal is gone from every listing and every screen and is still reachable by its id — which is the only way you are seeing this field. goals_update with `deleted: false` brings it back"
    )]
    pub deleted_unix_seconds: Option<i64>,
}

impl GoalView {
    pub fn from_model(goal: &GoalModel, project: &ProjectModel, board: &BoardInner) -> Self {
        let (tasks_amount, done_amount) = board.goal_progress(&goal.project_id, goal.number);

        Self {
            id: crate::board::compose_goal_handle(&project.prefix, goal.number),
            project: project.prefix.clone(),
            name: goal.name.clone(),
            description: goal.description.clone(),
            color: rust_extensions::AsStr::as_str(&goal.color).to_string(),
            priority: rust_extensions::AsStr::as_str(&goal.priority).to_string(),
            status: goal.status().to_string(),
            tasks_amount: tasks_amount as i32,
            done_amount: done_amount as i32,
            subtasks: SubtaskView::from_models(&goal.subtasks),
            documents: goal.documents.clone(),
            // Resolved through the board, which is what leaves a deleted release out and puts the rest in
            // the one order every list of releases is in.
            releases: board
                .releases_of_goal(goal)
                .iter()
                .map(|release| ReleaseView::from_model(release, project, board))
                .collect(),
            comments_amount: goal.comments.len() as i32,
            created_unix_seconds: goal.created.unix_microseconds / 1_000_000,
            updated_unix_seconds: goal.updated.unix_microseconds / 1_000_000,
            closed_unix_seconds: goal
                .close_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
            deleted_unix_seconds: goal
                .deleted_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
        }
    }
}

/// A moment a caller gave, as a tool hands it back: `2026-10-07T14:30:00Z`.
///
/// RFC 3339 in UTC, to the second — the spelling `releases_create` takes, so what a release reports is what
/// can be passed straight back. Every OTHER moment on this surface is unix seconds, and the difference is
/// deliberate: those are stamps this service put on, read by code; these two are statements somebody made
/// and an agent has to repeat to a person, and `1791376200` cannot be read aloud.
fn caller_moment_to_view(moment: rust_extensions::date_time::DateTimeAsMicroseconds) -> String {
    let stamp = moment.to_rfc3339_utc();

    match stamp.split_once('.') {
        Some((to_the_second, _)) => format!("{to_the_second}Z"),
        None => stamp,
    }
}

/// One microservice of a release, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ServiceReleaseView {
    #[property(
        description = "Which microservice — the name it is deployed under. The identity of this entry within its release: it is what releases_update finds the entry by, to correct it or to remove it"
    )]
    pub microservice_id: String,
    #[property(description = "Which version of it went out")]
    pub version: String,
    #[property(
        description = "The commit that version was built from, lower-case hex. The half of the entry that cannot have moved since: a tag can be re-pointed, this cannot"
    )]
    pub git_hash: String,
    #[property(
        description = "When this service went out, as `2026-10-07T14:30:00Z` (UTC). What whoever recorded the release SAID — nothing here checks it against a deploy"
    )]
    pub datetime: String,
    #[property(
        description = "WHAT HAS TO CHANGE IN THIS SERVICE'S SETTINGS for the release to work — a key to add, a value to change, a secret to supply. Empty means nothing changes. Kept apart from `description` because it is the one part of a release somebody has to ACT on while rolling it out: read it before deploying this version anywhere else"
    )]
    pub settings_update_note: String,
    #[property(
        description = "Anything else worth knowing about this service's part of the release, as Markdown. Often empty"
    )]
    pub description: String,
}

/// One release, as a tool sees it.
///
/// The record that a feature went out, and in what. Whole wherever it appears — in `releases_list` and on
/// the goal that lists it alike — because the services ARE the release: a view without them would be a
/// title and a date.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ReleaseView {
    #[property(
        description = "The release id, like `RMS-R12`. This is how a release is named everywhere — in releases_update, and in add_releases on a goal. The number comes from the same counter task and goal numbers come from, so `RMS-12`, `RMS-G12` and `RMS-R12` are never more than one real thing; the `R` is what says which kind you are holding"
    )]
    pub id: String,
    #[property(description = "The prefix of the project this release belongs to")]
    pub project: String,
    #[property(description = "What went out, in one line")]
    pub title: String,
    #[property(description = "What this release is, as Markdown — the context around it")]
    pub description: String,
    #[property(
        description = "What changed for whoever is on the receiving end, as Markdown. The part meant to be repeated to the people affected"
    )]
    pub release_notes: String,
    #[property(
        description = "The date of the release, as `2026-10-07T00:00:00Z` (UTC) — what its author gave, and what every list of releases is ordered by, newest first. Not when the record was written: that is `created_unix_seconds`, and a release is often written down after the fact"
    )]
    pub date: String,
    #[property(
        description = "One entry per microservice this release touched, in the order they were added: which version went out, built from which commit, and whether its settings have to change. A release with none is a title and a date — add them with releases_update"
    )]
    pub services: Vec<ServiceReleaseView>,
    #[property(
        description = "Ids of the goals that list this release — normally exactly one, the feature it shipped. Derived, not stored on the release: a goal says which releases it went out in. Empty means nobody has attached it to a goal yet"
    )]
    pub goals: Vec<String>,
    #[property(
        description = "TRUE ONCE THIS RELEASE IS OUT ON PRODUCTION. A release is recorded when it ships somewhere — usually a test stand first — so false does not mean it is nowhere, it means it has not reached prod. This is the field that answers 'is it live for users', and what `released_on_prod` on releases_list filters by. Set it with releases_update when the rollout to production has actually happened, not when it is planned"
    )]
    pub released_on_prod: bool,
    #[property(
        description = "When it was marked as on production, unix seconds (UTC); absent while it is not. Stamped by the server at the moment of the mark, so it is when somebody SAID it reached prod. Cleared when the mark is taken off"
    )]
    pub released_on_prod_unix_seconds: Option<i64>,
    #[property(
        description = "How many notes are on the release's thread — how the rollout went, what was noticed afterwards, why it was pulled back. Read them with releases_get_comments. The notes say what changed; the thread says what happened"
    )]
    pub comments_amount: i32,
    #[property(description = "When the release was written down, unix seconds (UTC)")]
    pub created_unix_seconds: i64,
    #[property(
        description = "When the record itself last changed, unix seconds (UTC). A comment does not move this"
    )]
    pub updated_unix_seconds: i64,
    #[property(
        description = "When it was DELETED, unix seconds (UTC), and absent for a release that is not. A deleted release is gone from every list and from every goal that listed it, and is still reachable by its id — which is the only way you are seeing this field. releases_update with `deleted: false` brings it back, onto those goals too"
    )]
    pub deleted_unix_seconds: Option<i64>,
}

impl ReleaseView {
    pub fn from_model(release: &ReleaseModel, project: &ProjectModel, board: &BoardInner) -> Self {
        Self {
            id: crate::board::compose_release_handle(&project.prefix, release.number),
            project: project.prefix.clone(),
            title: release.title.clone(),
            description: release.description.clone(),
            release_notes: release.release_notes.clone(),
            date: caller_moment_to_view(release.date),
            services: release
                .services
                .iter()
                .map(|itm| ServiceReleaseView {
                    microservice_id: itm.microservice_id.clone(),
                    version: itm.version.clone(),
                    git_hash: itm.git_hash.clone(),
                    datetime: caller_moment_to_view(itm.datetime),
                    settings_update_note: itm.settings_update_note.clone(),
                    description: itm.description.clone(),
                })
                .collect(),
            goals: board
                .goals_of_release(&release.project_id, release.number)
                .iter()
                .map(|goal| crate::board::compose_goal_handle(&project.prefix, goal.number))
                .collect(),
            released_on_prod: release.is_released_on_prod(),
            released_on_prod_unix_seconds: release
                .released_on_prod_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
            comments_amount: release.comments.len() as i32,
            created_unix_seconds: release.created.unix_microseconds / 1_000_000,
            updated_unix_seconds: release.updated.unix_microseconds / 1_000_000,
            deleted_unix_seconds: release
                .deleted_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
        }
    }
}

/// One microservice to put in a release, as a write tool takes it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ServiceReleaseInput {
    #[property(
        description = "Which microservice — the name it is deployed under, one word, like `my-service`. A release names a service ONCE: passing one it already has corrects that entry rather than adding a second"
    )]
    pub microservice_id: String,
    #[property(description = "Which version of it went out, like `1.2.3`. One word")]
    pub version: String,
    #[property(
        description = "The commit that version was built from — 7 to 64 hex characters, as `git rev-parse HEAD` prints it. It has to be a HASH: a branch or a tag is refused, because both can move and a release is a record of what cannot"
    )]
    pub git_hash: String,
    #[property(
        description = "When this service went out: `2026-10-07`, or a date and time like `2026-10-07T14:30:00Z`. A zone offset such as `+03:00` is honoured; no zone means UTC. Omit for now, which is right when you are recording it as it happens"
    )]
    pub datetime: Option<String>,
    #[property(
        description = "WHAT HAS TO CHANGE IN THIS SERVICE'S SETTINGS for the release to work, as Markdown — the key to add, the value to change, the secret to supply. Its own field on purpose: it is the part of a release somebody has to act on, and the board marks the services that carry one. Omit when the settings do not change. On a service the release already has, omitting it leaves the note alone and an empty string clears it"
    )]
    pub settings_update_note: Option<String>,
    #[property(
        description = "Anything else about this service's part of the release, as Markdown. NOT for settings changes — those go in settings_update_note. Omit when there is nothing to add; on a service the release already has, omitting it leaves what is there"
    )]
    pub description: Option<String>,
}

impl ServiceReleaseInput {
    /// The services a write tool was handed, as the write path takes them.
    pub fn into_new(src: Option<Vec<Self>>) -> Vec<crate::scripts::NewServiceRelease> {
        src.unwrap_or_default()
            .into_iter()
            .map(|itm| crate::scripts::NewServiceRelease {
                microservice_id: itm.microservice_id,
                version: itm.version,
                git_hash: itm.git_hash,
                datetime: itm.datetime,
                settings_update_note: itm.settings_update_note,
                description: itm.description,
            })
            .collect()
    }
}

/// The release-reference fields of a goal's write tool, gathered so the conversion lives in one place.
pub struct ReleaseOps {
    pub add: Option<Vec<String>>,
    pub remove: Option<Vec<String>>,
}

impl ReleaseOps {
    pub fn into_patch(self) -> crate::scripts::ReleasesPatch {
        crate::scripts::ReleasesPatch {
            add: self.add.unwrap_or_default(),
            remove: self.remove.unwrap_or_default(),
        }
    }
}

/// One checklist item, as a tool sees it. The same shape on a task and on a goal.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SubtaskView {
    #[property(
        description = "The item's id. This is what goes into check_subtasks, uncheck_subtasks, edit_subtasks and remove_subtasks — it is the ONLY way to name an item, since two items may read alike. Internal to the checklist: it is not a task id, it resolves nowhere else, and nobody is shown it"
    )]
    pub id: String,
    #[property(description = "The one-line title, which is what the board's card shows")]
    pub title: String,
    #[property(
        description = "The longer half, as Markdown — what the item actually involves. Often empty, which is normal for a one-line item"
    )]
    pub text: String,
    #[property(description = "Whether it has been ticked off")]
    pub done: bool,
}

impl SubtaskView {
    pub fn from_models(src: &[crate::board::SubtaskModel]) -> Vec<Self> {
        src.iter()
            .map(|itm| Self {
                id: itm.id.clone(),
                title: itm.title.clone(),
                text: itm.text.clone(),
                done: itm.done,
            })
            .collect()
    }
}

/// One checklist item to add, as a write tool takes it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SubtaskInput {
    #[property(
        description = "The one-line title — what the item is, short enough to read at a glance. This is all a person sees until they open the item"
    )]
    pub title: String,
    #[property(
        description = "The longer half, as Markdown: what the item actually involves, where to look, what to watch out for. Omit for a one-line item, which is the usual case"
    )]
    pub text: Option<String>,
}

impl SubtaskInput {
    /// The items a create tool was handed, as the write path takes them.
    pub fn into_new(src: Option<Vec<Self>>) -> Vec<crate::scripts::NewSubtask> {
        src.unwrap_or_default()
            .into_iter()
            .map(|itm| crate::scripts::NewSubtask {
                title: itm.title,
                text: itm.text,
            })
            .collect()
    }
}

/// A rewrite of one existing checklist item.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct SubtaskEditInput {
    #[property(
        description = "Which item, by the `id` its checklist reported. Not its title — two items may read alike"
    )]
    pub id: String,
    #[property(description = "Reword the title. Omit to leave it as it is")]
    pub title: Option<String>,
    #[property(
        description = "Rewrite the longer half. Pass an empty string to clear it; omit to leave it as it is"
    )]
    pub text: Option<String>,
}

/// The checklist fields of a write tool, gathered so a task and a goal convert them the same way.
///
/// Named fields rather than five positional arguments: three of them are `Option<Vec<String>>` and would be
/// silently swappable, which for check / uncheck / remove is the worst possible mix-up.
pub struct SubtaskOps {
    pub add: Option<Vec<SubtaskInput>>,
    pub edit: Option<Vec<SubtaskEditInput>>,
    pub check: Option<Vec<String>>,
    pub uncheck: Option<Vec<String>>,
    pub remove: Option<Vec<String>>,
}

impl SubtaskOps {
    pub fn into_patch(self) -> crate::scripts::SubtasksPatch {
        crate::scripts::SubtasksPatch {
            add: SubtaskInput::into_new(self.add),
            edit: self
                .edit
                .unwrap_or_default()
                .into_iter()
                .map(|itm| crate::scripts::SubtaskEdit {
                    id: itm.id,
                    title: itm.title,
                    text: itm.text,
                })
                .collect(),
            check: self.check.unwrap_or_default(),
            uncheck: self.uncheck.unwrap_or_default(),
            remove: self.remove.unwrap_or_default(),
        }
    }
}

/// The document-reference fields of a write tool, gathered so a task and a goal convert them the same way.
pub struct DocumentOps {
    pub add: Option<Vec<String>>,
    pub remove: Option<Vec<String>>,
}

impl DocumentOps {
    pub fn into_patch(self) -> crate::scripts::DocumentsPatch {
        crate::scripts::DocumentsPatch {
            add: self.add.unwrap_or_default(),
            remove: self.remove.unwrap_or_default(),
        }
    }
}

/// One build a task produced, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GhActionView {
    #[property(
        description = "The url of the run — a link a person can open, and the way this build is named when you remove it"
    )]
    pub url: String,
    #[property(
        description = "What it is called, e.g. `my-service v1.2.3`. Never empty: when nobody named it, one was worked out from the url"
    )]
    pub title: String,
    #[property(
        description = "When the build was ATTACHED to the task, unix seconds (UTC) — not when GitHub ran it. Nothing here reads GitHub"
    )]
    pub moment_unix_seconds: i64,
}

impl GhActionView {
    pub fn from_models(src: &[crate::board::GhActionModel]) -> Vec<Self> {
        src.iter()
            .map(|itm| Self {
                url: itm.url.clone(),
                title: itm.title.clone(),
                moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
            })
            .collect()
    }
}

/// One build link to attach, as a write tool takes it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GhActionInput {
    #[property(
        description = "The url of the GitHub Actions run, like `https://github.com/<owner>/<repo>/actions/runs/<id>`. Any http(s) link is accepted — an enterprise install is not on github.com — but it has to be a LINK: a run number on its own is refused, because a reference nobody can open is not a record of a build"
    )]
    pub url: String,
    #[property(
        description = "What to call it — what shipped and which version, e.g. `my-service v1.2.3`. This is what the card draws, so write what a person scanning the task would want to read. Omit it and one is worked out from the url (`my-service #18423`), which is a fallback rather than a good name"
    )]
    pub title: Option<String>,
}

/// The build-link fields of a write tool, gathered so the conversion lives in one place.
pub struct GhActionOps {
    pub add: Option<Vec<GhActionInput>>,
    pub remove: Option<Vec<String>>,
}

impl GhActionOps {
    pub fn into_patch(self) -> crate::scripts::GhActionsPatch {
        crate::scripts::GhActionsPatch {
            add: self
                .add
                .unwrap_or_default()
                .into_iter()
                .map(|itm| crate::scripts::NewGhAction {
                    url: itm.url,
                    title: itm.title,
                })
                .collect(),
            remove: self.remove.unwrap_or_default(),
        }
    }
}

/// One document, as a tool sees it — WITHOUT its text.
///
/// The split matters here more than anywhere else on this surface: a document can be a whole specification,
/// and a list of them is drawn from paths. So listing documents returns these, and reading one returns its
/// content. An agent that dumped every document into its context to find one would have spent the context it
/// needed for the work.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentView {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — a url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, it is taken by documents_get wherever an id is, and it survives being pasted into a CLAUDE.md or an issue, because it names its own board. It is also the live address of the bytes, give or take a leading slash"
    )]
    pub reference: String,
    #[property(
        description = "The document's id. For one of the project's own it is minted at creation and never changes — not when the text is rewritten, not when the path moves — which is what makes the history complete. For a file in a connected repository it is derived from where the file IS, so it changes when the file moves and is gone when the file is: such a file has no id of its own, because it is a working copy rather than a row. Prefer `reference` for anything you are storing or writing down"
    )]
    pub id: String,
    #[property(description = "The prefix of the project this document belongs to")]
    pub project: String,
    #[property(
        description = "Where it lives, e.g. `docs/design/system.md`. This is its name AND its position: folders are not stored anywhere, they are read off these paths. A path is unique within a project, so uploading to a path that is taken writes a new version of what is there"
    )]
    pub path: String,
    #[property(
        description = "Which version this is. Starts at 1 and moves on every write of any kind — a rewrite, a move, a delete, a restore — so it is also how many history entries the document has"
    )]
    pub version: i64,
    #[property(
        description = "What it IS — a MIME type: `text/markdown`, `application/pdf`. Worked out from the path when nobody says otherwise, so `docs/spec.pdf` is a PDF without being told"
    )]
    pub content_type: String,
    #[property(
        description = "True when the document is a FILE rather than text — a PDF, an image. documents_get returns its bytes base64-encoded, and nothing about it can be diffed or searched. False for anything you can read"
    )]
    pub is_binary: bool,
    #[property(
        description = "How big it is, in BYTES — for text and files alike, so a listing is comparable. Read it before reading the document: it is the difference between a note and a specification, and between a note and a 10 MB file"
    )]
    pub size: i64,
    #[property(description = "When it was first created, unix seconds (UTC)")]
    pub created_unix_seconds: i64,
    #[property(description = "When the current version was written, unix seconds (UTC)")]
    pub updated_unix_seconds: i64,
    #[property(description = "Who wrote the current version — an email, or `AI`")]
    pub updated_by: String,
    #[property(
        description = "The hash of the content — 64 hex characters, or empty for a file, which is never briefed. It is what a brief is filed under: documents_set_brief takes this, and every copy of the same text anywhere on this server carries the same one, so briefing through any of them briefs all of them. It changes whenever the text does, which is what makes an edited document unbriefed again"
    )]
    pub content_hash: String,
    #[property(
        description = "WHAT IS IN IT, in a few sentences somebody wrote after reading it — empty when nobody has yet. THIS IS THE FIELD TO SCAN. A listing of a board is dozens of paths that say almost nothing; the briefs beside them say what each document covers, so you open the two that matter instead of five that do not. An empty brief is not an empty document, it is an unread one: documents_next_without_brief hands them over one at a time"
    )]
    pub brief: String,
}

impl DocumentView {
    /// From a whole row, which a write has just produced.
    ///
    /// The brief is passed in rather than looked up: this type knows nothing about the app, and a
    /// signature that asks for it is what makes the compiler point at every place a listing could have
    /// forgotten one.
    pub fn from_dto(src: &crate::postgres::DocumentDto, project_prefix: &str, brief: String) -> Self {
        let body = crate::scripts::body_of(src);

        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id.clone(),
            project: project_prefix.to_string(),
            path: src.doc_path.clone(),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            is_binary: body.is_binary(),
            size: body.size_bytes(),
            version: src.version,
            created_unix_seconds: src.created.unix_microseconds / 1_000_000,
            updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
            updated_by: src.updated_by.clone(),
            content_hash: src.content_hash.clone().unwrap_or_default(),
            brief,
        }
    }

    /// From the in-memory index, which is what a listing reads — no payload involved at all.
    pub fn from_entry(
        src: &crate::documents::DocumentIndexEntry,
        project_prefix: &str,
        brief: String,
    ) -> Self {
        Self {
            // Off the ID rather than off the path, because the id is what says which KIND this is: a
            // mirrored file carries a `github:` one and becomes a `raw/{project}/github/…` reference, and
            // everything else is a row and becomes `raw/{project}/document/{id}`.
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id.clone(),
            project: project_prefix.to_string(),
            path: src.path.clone(),
            content_type: src.content_type.clone(),
            is_binary: src.is_binary,
            size: src.size,
            version: src.version,
            created_unix_seconds: src.created.unix_microseconds / 1_000_000,
            updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
            updated_by: src.updated_by.clone(),
            content_hash: src.content_hash.clone().unwrap_or_default(),
            brief,
        }
    }
}

/// One document, with its text. What reading a document returns.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentContentView {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(description = "The document's id — stable for the life of the document")]
    pub id: String,
    #[property(description = "The prefix of the project this document belongs to")]
    pub project: String,
    #[property(description = "Where it lives")]
    pub path: String,
    #[property(
        description = "Which version this text is. Not necessarily the current one: documents_history can hand back an older version, and this says which"
    )]
    pub version: i64,
    #[property(
        description = "What it IS — a MIME type. `text/*` and a few `application/*` types are text; everything else is a file"
    )]
    pub content_type: String,
    #[property(
        description = "True when this document is a FILE rather than text. Then `content` is absent and `content_base64` carries it"
    )]
    pub is_binary: bool,
    #[property(
        description = "How big the WHOLE document is, in bytes — not how much of it is in `content`. When you asked for a slice those two differ, and this is the one to compare a byte budget against"
    )]
    pub size: i64,
    #[property(
        description = "How many lines the whole document has. Absent for a file. This is the number `from_line` and `to_line` are bounded by, and the cheapest way to tell whether a document is worth an outline before reading it"
    )]
    pub lines_total: Option<i64>,
    #[property(
        description = "The first line `content` holds, 1-based — present only when you asked for a slice. Together with `to_line` it says exactly which part of the document you are looking at, so a quote from it can be cited by line"
    )]
    pub from_line: Option<i64>,
    #[property(
        description = "The last line returned WHOLE, 1-based and inclusive. Present only for a slice. It may be before the `to_line` you asked for, when `max_bytes` ran out first — and it is `from_line - 1` when the budget could not fit even one line, so `content` holds part of a line that no number claims. READ ON FROM `to_line + 1` either way: that re-reads a partially shown line instead of skipping the half you were not given"
    )]
    pub to_line: Option<i64>,
    #[property(
        description = "True when `content` is NOT the whole document — a line range, a byte budget, or both. Never edit from a truncated read and never conclude a document does not mention something from one: what you did not see is exactly what you cannot reason about"
    )]
    pub truncated: bool,
    #[property(
        description = "The document as text, when it IS text — Markdown usually. Absent for a file, which arrives in `content_base64` instead. Holds only the requested slice when you asked for one — `from_line`, `to_line` and `truncated` say what you got"
    )]
    pub content: Option<String>,
    #[property(
        description = "The document as base64, when it is a FILE. base64 because JSON cannot carry bytes and there is no other way to hand you a PDF through a tool call — it is NOT how it is stored, and the browser never sees base64 at all. Absent for a text document. Remember it is a third larger than the file: check `size` first"
    )]
    pub content_base64: Option<String>,
    #[property(description = "When this version was written, unix seconds (UTC)")]
    pub updated_unix_seconds: i64,
    #[property(description = "Who wrote this version — an email, or `AI`")]
    pub updated_by: String,
    #[property(
        description = "The hash of the content you are reading — 64 hex characters, or empty for a file. Pass it to documents_set_brief with what you learned: that is the whole of the briefing loop, and it means the next reader of this text on any board finds your brief instead of reading it again"
    )]
    pub content_hash: String,
    #[property(
        description = "What somebody already wrote about this document, or empty. Worth reading before the text: if it is there and it answers your question, you have saved yourself the document"
    )]
    pub brief: String,
}

impl DocumentContentView {
    pub fn from_dto(
        src: &crate::postgres::DocumentDto,
        project_prefix: &str,
        brief: String,
    ) -> Self {
        let (content, content_base64, is_binary, size) = split_body(crate::scripts::body_of(src));

        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id.clone(),
            project: project_prefix.to_string(),
            path: src.doc_path.clone(),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            is_binary,
            size,
            lines_total: content.as_deref().map(count_lines),
            from_line: None,
            to_line: None,
            truncated: false,
            version: src.version,
            content,
            content_base64,
            updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
            updated_by: src.updated_by.clone(),
            content_hash: src.content_hash.clone().unwrap_or_default(),
            brief,
        }
    }

    /// An OLD version, read out of the history.
    ///
    /// The same shape as the current one on purpose: a caller reading version 3 wants the text and where it
    /// lived, and should not have to handle a second type to get it.
    pub fn from_history(
        src: &crate::postgres::DocumentHistoryDto,
        project_prefix: &str,
        // A brief is filed under the hash of a CONTENT, so an old version has one exactly when somebody
        // briefed that text — usually because it was the current one at the time. Looked up by the caller,
        // which has the app; computed here, which has the payload.
        brief_of: impl FnOnce(&str) -> String,
    ) -> Self {
        let (content, content_base64, is_binary, size) =
            split_body(crate::scripts::body_of_version(src));

        // Computed rather than read: a history row keeps no hash column — it needs none, because the text
        // is right here and hashing it is cheaper than a column that could disagree with it.
        let content_hash = content
            .as_deref()
            .map(|itm| crate::documents::content_hash(itm.as_bytes()))
            .unwrap_or_default();

        let brief = match content_hash.is_empty() {
            true => String::new(),
            false => brief_of(&content_hash),
        };

        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.document_id,
            ),
            id: src.document_id.clone(),
            project: project_prefix.to_string(),
            // The path AT THAT VERSION, not the current one — which is the whole reason a move is recorded
            // as a version.
            path: src.doc_path.clone(),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            is_binary,
            size,
            lines_total: content.as_deref().map(count_lines),
            from_line: None,
            to_line: None,
            truncated: false,
            version: src.version,
            content,
            content_base64,
            updated_unix_seconds: src.moment.unix_microseconds / 1_000_000,
            updated_by: src.who.clone(),
            content_hash,
            brief,
        }
    }

    /// Narrow the text to the slice that was asked for.
    ///
    /// Applied to a built view rather than threaded through both constructors above: they differ in where
    /// every OTHER field comes from, and slicing is the one step that depends on none of it. `size` is left
    /// alone on purpose — it is how big the document is, which is exactly what a caller deciding whether to
    /// read the rest of it needs, and it would be useless as a measure of what came back.
    ///
    /// **A file is refused rather than sliced.** Lines are a property of text; the bytes of a PDF have none,
    /// and half a PDF is not a smaller PDF. Cutting one by `max_bytes` would hand back base64 that decodes
    /// to a corrupt file, which is worse than a refusal because it looks like it worked.
    pub fn into_slice(
        mut self,
        from_line: Option<i64>,
        to_line: Option<i64>,
        max_bytes: Option<i64>,
    ) -> Result<Self, String> {
        if from_line.is_none() && to_line.is_none() && max_bytes.is_none() {
            return Ok(self);
        }

        let Some(text) = self.content.as_deref() else {
            return Err(format!(
                "document {} is a file ({}), and a file has no lines to slice — `from_line`, `to_line` and \
                 `max_bytes` only apply to text. Read `size` and fetch it whole, or open it in the browser",
                self.id, self.content_type
            ));
        };

        let slice = crate::scripts::slice_text(text, from_line, to_line, max_bytes)?;

        self.content = Some(slice.text);
        self.lines_total = Some(slice.lines_total);
        self.from_line = Some(slice.from_line);
        self.to_line = Some(slice.to_line);
        self.truncated = slice.truncated;

        Ok(self)
    }
}

/// How many lines a text has, counted the same way everything else here counts them.
fn count_lines(text: &str) -> i64 {
    text.lines().count() as i64
}

/// A payload as the two optional wire fields: `(content, content_base64, is_binary, size)`.
///
/// One function because both constructors above need the identical four-way split, and because base64 belongs
/// in exactly one place on this surface — here, at the boundary JSON forces it through.
fn split_body(
    body: crate::scripts::DocumentBody,
) -> (Option<String>, Option<String>, bool, i64) {
    let size = body.size_bytes();

    match body {
        crate::scripts::DocumentBody::Text(text) => (Some(text), None, false, size),
        crate::scripts::DocumentBody::Binary(bytes) => {
            use rust_extensions::base64::IntoBase64;
            (None, Some(bytes.into_base64()), true, size)
        }
    }
}

/// One line of a document that matched a search, with the lines around it.
///
/// The unit of a search result is a LINE and not a document, which is the whole difference between this and
/// reading the document: a caller learns where the thing is said without paying for everything else it says.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentMatchView {
    #[property(
        description = "Which line, 1-based. Hand it straight back to documents_get as `from_line` to read around it — the numbering is the same one documents_outline reports"
    )]
    pub line_number: i64,
    #[property(
        description = "The line itself. Clipped with a `…` when it is very long, so one minified line cannot become most of the answer"
    )]
    pub line: String,
    #[property(
        description = "The lines immediately before it, oldest first — as many as `context_lines` asked for, fewer at the top of a document"
    )]
    pub context_before: Vec<String>,
    #[property(description = "The lines immediately after it, in order")]
    pub context_after: Vec<String>,
}

/// One document a search found something in.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentSearchHitView {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(description = "The document's id — what documents_get, documents_edit and documents_outline take")]
    pub id: String,
    #[property(description = "Where it lives")]
    pub path: String,
    #[property(
        description = "How many LINES of this document matched in total. May be more than `matches` holds — that is what `max_matches_per_document` did, and it is the number that tells a passing mention from the document the subject actually lives in"
    )]
    pub matches_total: i32,
    #[property(description = "The matching lines, in the order they appear in the document")]
    pub matches: Vec<DocumentMatchView>,
}

impl DocumentSearchHitView {
    pub fn from_hit(src: crate::scripts::DocumentSearchHit, project_prefix: &str) -> Self {
        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id,
            path: src.path,
            matches_total: src.matches_total,
            matches: src
                .matches
                .into_iter()
                .map(|itm| DocumentMatchView {
                    line_number: itm.line_number,
                    line: itm.line,
                    context_before: itm.context_before,
                    context_after: itm.context_after,
                })
                .collect(),
        }
    }
}

/// One heading of a document, and the span it opens.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentHeadingView {
    #[property(description = "1 for `#`, 2 for `##`, and so on down to 6")]
    pub level: i32,
    #[property(description = "The heading text, with the hashes and any closing hashes stripped")]
    pub title: String,
    #[property(description = "The line the heading itself is on, 1-based")]
    pub line: i64,
    #[property(
        description = "The last line of this section, inclusive. READ THE SECTION with documents_get and `from_line: line`, `to_line: end_line` — that pairing is what this tool exists for"
    )]
    pub end_line: i64,
    #[property(
        description = "How big the section is in bytes, INCLUDING its subsections — a section runs to the next heading at its level or above, so the spans of nested headings overlap on purpose. Check it before slicing: a section can be most of the document"
    )]
    pub size: i64,
}

/// One entry of a document's history: what was done to it, by whom, and where it was at the time.
///
/// The text is left out on purpose — a history of a document rewritten fifty times would otherwise be fifty
/// copies of it. `documents_history` with a `version` hands back the one text that is wanted.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DocumentVersionView {
    #[property(description = "Which version this entry is. They run 1..n with no gaps")]
    pub version: i64,
    #[property(
        description = "What happened: `created`, `updated` (the text was rewritten), `moved` (the path changed, the text did not), `deleted` (it went to the trash) or `restored`"
    )]
    pub event: String,
    #[property(
        description = "The path the document had AT THIS VERSION. This is what answers 'when did it move, and from where' — the reason a move is a version of its own"
    )]
    pub path: String,
    #[property(description = "How big the payload was at this version, in bytes")]
    pub size: i64,
    #[property(description = "What it was at this version — a MIME type. A rewrite may change it")]
    pub content_type: String,
    #[property(description = "Who did it — an email, or `AI`")]
    pub who: String,
    #[property(description = "When, unix seconds (UTC)")]
    pub moment_unix_seconds: i64,
}

impl DocumentVersionView {
    /// From the blob-free history shape: listing a history must not read twenty copies of the document.
    pub fn from_dto(src: &crate::postgres::DocumentVersionDto) -> Self {
        Self {
            version: src.version,
            event: src.event.clone(),
            path: src.doc_path.clone(),
            size: src.content_size.unwrap_or(0),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            who: src.who.clone(),
            moment_unix_seconds: src.moment.unix_microseconds / 1_000_000,
        }
    }
}

/// One document a folder deletion took.
///
/// Two fields and no more, because a bulk answer is read differently from a single one: what a caller does
/// with it is either recognise a path they did not mean to delete, or hand an id back to
/// `documents_restore`. Sizes and content types would be forty lines of context nobody asked for.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct DeletedDocumentView {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(
        description = "The document's id, unchanged by the deletion — this is what documents_restore takes"
    )]
    pub id: String,
    #[property(
        description = "Where it was. A restore puts it back here, and the path is free from the moment it went"
    )]
    pub path: String,
}

impl DeletedDocumentView {
    pub fn from_dto(src: &crate::scripts::DeletedDocument, project_prefix: &str) -> Self {
        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id.clone(),
            path: src.path.clone(),
        }
    }
}

/// One document in the trash.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TrashedDocumentView {
    #[property(
        description = "WHAT TO WRITE DOWN AND WHAT TO ATTACH — the url naming this document: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. It goes into add_documents on a task or a goal, and every tool here takes it wherever it takes an id"
    )]
    pub reference: String,
    #[property(
        description = "The document's id — unchanged by the deletion, which is what makes restoring it put the same document back and every reference to it work again"
    )]
    pub id: String,
    #[property(
        description = "The path it had when it was deleted. A restore puts it back here unless you name another path, and is refused if something has taken this one since"
    )]
    pub path: String,
    #[property(description = "How big it is, in bytes — the payload is kept in full")]
    pub size: i64,
    #[property(description = "What it is — a MIME type")]
    pub content_type: String,
    #[property(description = "When it was deleted, unix seconds (UTC)")]
    pub deleted_unix_seconds: i64,
    #[property(description = "Who deleted it — an email, or `AI`")]
    pub deleted_by: String,
}

impl TrashedDocumentView {
    pub fn from_dto(src: &crate::postgres::DocumentTrashIndexDto, project_prefix: &str) -> Self {
        Self {
            reference: task_manager_shared::documents::canonical_document_reference(
                project_prefix,
                &src.id,
            ),
            id: src.id.clone(),
            path: src.doc_path.clone(),
            size: src.content_size.unwrap_or(0),
            content_type: crate::scripts::content_type_of(src.content_type.as_deref(), &src.doc_path),
            deleted_unix_seconds: src.deleted.unix_microseconds / 1_000_000,
            deleted_by: src.deleted_by.clone(),
        }
    }
}

/// One line of a card that matched a board search, and which part of the card it was on.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct BoardMatchView {
    #[property(
        description = "Where on the card this line is: `text` (the task's own text, or a goal's name and description — what the work IS), `comment` (a note on the thread — what somebody SAID about it, usually the more useful of the two), or `subtask` (a checklist item)"
    )]
    pub part: String,
    #[property(
        description = "Who wrote the comment — an email, or `AI`. Present only for a `comment`; the other parts belong to the card rather than to a person"
    )]
    pub who: Option<String>,
    #[property(description = "When the comment was left, unix seconds (UTC). Present only for a `comment`")]
    pub moment_unix_seconds: Option<i64>,
    #[property(description = "The matching line itself, clipped when very long")]
    pub line: String,
}

/// One task or goal a board search found something on.
///
/// Deliberately NOT a `TaskView`: a search returning whole tasks would cost what listing the board costs,
/// which is the thing this tool exists to avoid. Enough to decide which card to open, and then `tasks_list`
/// or `tasks_get_comments` opens it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct BoardSearchHitView {
    #[property(
        description = "The handle — `RMS-42` for a task, `RMS-G7` for a goal. This is what every other tool takes"
    )]
    pub id: String,
    #[property(
        description = "True when this is a GOAL rather than a task. Worth reading first when both matched: a goal is the container the work was organised under, so its thread is usually where the decision was made"
    )]
    pub is_goal: bool,
    #[property(
        description = "The first line of the text — what the card reads as at a glance, so you can pick without a second call"
    )]
    pub headline: String,
    #[property(description = "Which column the task is in, or `todo` / `done` for a goal")]
    pub status: String,
    #[property(description = "How urgent it is: `super-high`, `high`, `normal`, `low` or `super-low`")]
    pub priority: String,
    #[property(description = "The goal this task belongs to, by goal id. Absent on a goal and on a standalone task")]
    pub goal: Option<String>,
    #[property(description = "Who is on it — an email or `AI`. Absent for a goal and for unassigned work")]
    pub assignee: Option<String>,
    #[property(
        description = "How many lines matched on this card in total, across its text, its checklist and its whole thread. What the results are SORTED BY, and the number that tells the card where something was decided from the card that mentions it once"
    )]
    pub matches_total: i32,
    #[property(
        description = "The matching lines — at most a handful per card. When `matches_total` is bigger, read the thread with tasks_get_comments or goals_get_comments rather than searching again"
    )]
    pub matches: Vec<BoardMatchView>,
    #[property(
        description = "How many comments are on the thread altogether, matching or not. The thread is where the reasoning lives"
    )]
    pub comments_amount: i32,
    #[property(
        description = "True when this is closed longer ago than the project's archive window, so the board does not draw it. You are only seeing it because `include_archived` was asked for"
    )]
    pub is_archived: bool,
    #[property(
        description = "True when it has been deleted. Only ever present when `include_deleted` was asked for — a deleted card is off every board and every count"
    )]
    pub is_deleted: bool,
}

impl BoardSearchHitView {
    pub fn from_hit(src: crate::scripts::BoardHit) -> Self {
        Self {
            id: src.id,
            is_goal: src.is_goal,
            headline: src.headline,
            status: src.status,
            priority: src.priority,
            goal: src.goal,
            assignee: src.assignee,
            matches_total: src.matches_total,
            matches: src
                .matches
                .into_iter()
                .map(|itm| BoardMatchView {
                    part: itm.part.as_str().to_string(),
                    who: itm.who,
                    moment_unix_seconds: itm.moment_unix_seconds,
                    line: itm.line,
                })
                .collect(),
            comments_amount: src.comments_amount,
            is_archived: src.is_archived,
            is_deleted: src.is_deleted,
        }
    }
}

/// One comment on a task's thread.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct CommentView {
    #[property(description = "When it was left, unix seconds (UTC)")]
    pub moment_unix_seconds: i64,
    #[property(description = "Who left it — an email, or `AI`")]
    pub who: String,
    #[property(description = "The comment, as Markdown")]
    pub text: String,
}

/// One task, as a tool sees it.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TaskView {
    #[property(
        description = "The task id, like `RMS-42`. This is the only way to name a task — never its text, since two tasks may read alike. The older padded spelling `RMS-000042` is still accepted wherever an id is taken"
    )]
    pub id: String,
    #[property(description = "The prefix of the project this task is on")]
    pub project: String,
    #[property(description = "What the task says, as Markdown")]
    pub text: String,
    #[property(
        description = "Which column it sits in. A task whose stored status names a column the project no longer has reads as `todo`"
    )]
    pub status: String,
    #[property(
        description = "How urgent it is: `super-high`, `high`, `normal`, `low` or `super-low`. This is what decides where the card sits in its column — the board draws the most urgent at the top — and it is why tasks_list comes back in that order. `normal` is what most work is and what an unranked task reads as"
    )]
    pub priority: String,
    #[property(description = "What kind of work it is, or absent when it has no kind")]
    pub kind: Option<String>,
    #[property(
        description = "The goal this task is part of, by goal id — `RMS-G7`. Absent means the task stands on its own, which is a normal state and not an unfinished one. Work is organised by goal: when you are asked what is happening with something, this is the thread to pull"
    )]
    pub goal: Option<String>,
    #[property(description = "What that goal is called, absent for a standalone task")]
    pub goal_name: Option<String>,
    #[property(description = "Who is on it — an email, or `AI`. Absent means nobody yet")]
    pub assignee: Option<String>,
    #[property(
        description = "The assignee's name, when they are on the roster. `AI` resolves to itself; absent for an email with no user"
    )]
    pub assignee_name: Option<String>,
    #[property(description = "Tags on this task, lowercased and sorted")]
    pub labels: Vec<String>,
    #[property(
        description = "Ids of the tasks blocking this one. Always tasks of the same project — dependencies do not cross projects"
    )]
    pub depends_on: Vec<String>,
    #[property(
        description = "THE OTHER DIRECTION, and derived rather than stored: ids of the tasks that name THIS one in their `depends_on`, so finishing this task is what unblocks them. Check it before parking or re-scoping something: `depends_on` says what is in your way, `blocks` says who is waiting on you, and only the second is invisible from the task itself"
    )]
    pub blocks: Vec<String>,
    #[property(
        description = "Derived, not stored: true while any task in `depends_on` is not `done` — including an id matching no task at all, so a mistyped or deleted blocker keeps the task blocked rather than silently freeing it. Do NOT start a blocked task"
    )]
    pub blocked: bool,
    #[property(
        description = "The task's checklist, in the order it was written — the breakdown of THIS piece of work, kept inside it. Read it before starting: it says what the task actually involves, and ticking items off with tasks_update as you go is how the next reader sees where you got to. It is not a list of tasks: nothing here has a status, an assignee or a place on the board, and an unticked item does not stop the task from landing. Work somebody else has to see or depend on is a task of its own, under the same goal. Usually empty"
    )]
    pub subtasks: Vec<SubtaskView>,
    #[property(
        description = "References to the documents this task points at, each a url: `raw/{project}/document/{id}` for one of the project's own, `raw/{project}/github/{repository}/{path}` for a file in a connected repository. Read one by handing it to documents_get, which takes a reference wherever it takes an id. Read them BEFORE starting the task: a document attached to a piece of work is usually the specification for it. A reference here may have gone stale, since deleting a document moves it to the trash and a reference is never quietly dropped"
    )]
    pub documents: Vec<String>,
    #[property(
        description = "The builds this task produced, oldest first — GitHub Actions runs, each a url and what it is called. THE OTHER DIRECTION FROM `documents`: a document is what the work was done against, a build is what came out of it. Empty for most tasks; when it is not, this is what actually shipped from this piece of work, and it is the first thing to look at when somebody asks whether a change is out"
    )]
    pub gh_actions: Vec<GhActionView>,
    #[property(description = "How many comments are on the thread")]
    pub comments_amount: i32,
    #[property(description = "When the task was created, unix seconds (UTC)")]
    pub created_unix_seconds: i64,
    #[property(
        description = "When the task itself last changed, unix seconds (UTC). A comment does not move this — the thread is a separate record from the work"
    )]
    pub updated_unix_seconds: i64,
    #[property(
        description = "When the task landed in `done`, unix seconds (UTC), and absent whenever it is not there. Work closed more than seven days ago is archived and left out of tasks_list unless you ask for it"
    )]
    pub closed_unix_seconds: Option<i64>,
    #[property(
        description = "When it was DELETED, unix seconds (UTC), and absent for a task that is not. A deleted task is gone from every board, listing and count and is still reachable by its id — which is the only way you are seeing this field. tasks_update with `deleted: false` brings it back"
    )]
    pub deleted_unix_seconds: Option<i64>,
}

impl TaskView {
    pub fn from_model(task: &TaskModel, project: &ProjectModel, board: &BoardInner) -> Self {
        let goal = board.effective_goal(task);

        Self {
            id: compose_task_handle(&project.prefix, task.number),
            project: project.prefix.clone(),
            text: task.text.clone(),
            status: project.effective_status(&task.status),
            priority: rust_extensions::AsStr::as_str(&task.priority).to_string(),
            kind: project.effective_kind(task.kind.as_deref()),
            // Resolved rather than echoed, so a number naming no goal reads as standalone instead of as
            // an id the caller cannot look up.
            goal: goal
                .as_ref()
                .map(|itm| crate::board::compose_goal_handle(&project.prefix, itm.number)),
            goal_name: goal.as_ref().map(|itm| itm.name.clone()),
            assignee: task.assignee.clone(),
            assignee_name: task
                .assignee
                .as_ref()
                .and_then(|itm| board.display_name_of(itm)),
            labels: task.labels.clone(),
            depends_on: task
                .depends_on
                .iter()
                .map(|number| compose_task_handle(&project.prefix, *number))
                .collect(),
            blocks: board
                .blocks(&task.project_id, task.number)
                .iter()
                .map(|number| compose_task_handle(&project.prefix, *number))
                .collect(),
            blocked: board.is_blocked(task),
            subtasks: SubtaskView::from_models(&task.subtasks),
            documents: task.documents.clone(),
            gh_actions: GhActionView::from_models(&task.gh_actions),
            comments_amount: task.comments.len() as i32,
            created_unix_seconds: task.created.unix_microseconds / 1_000_000,
            updated_unix_seconds: task.updated.unix_microseconds / 1_000_000,
            closed_unix_seconds: task
                .close_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
            deleted_unix_seconds: task
                .deleted_moment
                .map(|itm| itm.unix_microseconds / 1_000_000),
        }
    }
}

/// One person on the roster.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct UserView {
    #[property(
        description = "The email. THIS is what goes into a task's `assignee` and a comment's `who` — never the name"
    )]
    pub email: String,
    #[property(
        description = "How this person is called in conversation. Match a spoken first name against this to find the email"
    )]
    pub name: String,
    #[property(
        description = "True when this person can no longer sign in. Do not assign new work to them; their existing tasks and comments are left as they are"
    )]
    pub disabled: bool,
    #[property(
        description = "True for the one entry that is not a person: the reserved assignee AI, meaning an agent does this task. Assignable on every board and never disabled. Put it in `assignee` when the work is yours"
    )]
    pub reserved: bool,
}

impl UserView {
    pub fn from_model(user: &UserModel) -> Self {
        Self {
            email: user.email.clone(),
            name: user.name.clone(),
            disabled: user.disabled,
            reserved: false,
        }
    }

    /// The reserved assignee, which has no user row to build from.
    pub fn ai() -> Self {
        Self {
            email: task_manager_shared::users::ASSIGNEE_AI.to_string(),
            name: "AI".to_string(),
            disabled: false,
            reserved: true,
        }
    }
}
