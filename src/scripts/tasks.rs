use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::projects::{COLUMN_ID_DONE, COLUMN_ID_TODO};

use crate::app::AppContext;
use crate::board::{ProjectModel, TaskModel};
use crate::mappers::parse_dependency;
use crate::postgres::TaskDto;

use super::resolve::{resolve_project_by_prefix, resolve_task};

/// What a caller wants to change about a task. Every field is optional and `None` means "leave it".
///
/// The distinction that matters is between `None` and an *empty* value: `assignee: Some("")` clears
/// the assignee, `depends_on: Some(vec![])` clears the dependencies. Without that, there would be no
/// way to express "remove this" at all.
#[derive(Default)]
pub struct TaskPatch {
    pub text: Option<String>,
    pub status: Option<String>,
    /// Re-rank it. `None` leaves the priority alone; there is no "clear it", because every task has one and
    /// what a caller means by none is `normal`.
    pub priority: Option<String>,
    pub kind: Option<String>,
    /// Move it under a goal, by goal id. An empty string detaches it; `None` leaves it where it is.
    pub goal: Option<String>,
    pub assignee: Option<String>,
    pub add_labels: Vec<String>,
    pub remove_labels: Vec<String>,
    pub depends_on: Option<Vec<String>>,
    /// What to do to the checklist: add items, reword them, tick them, remove them. Never a whole-list
    /// replacement — see [`super::SubtasksPatch`].
    pub subtasks: super::SubtasksPatch,
    /// Which documents this task points at: ids to attach, ids to detach. Add/remove rather than a whole
    /// list, for the same reason labels are — see [`super::DocumentsPatch`].
    pub documents: super::DocumentsPatch,
    /// Which builds came out of this task: links to attach, links to detach. Add/remove for the same reason
    /// again — see [`super::GhActionsPatch`].
    pub gh_actions: super::GhActionsPatch,
    /// `Some(false)` brings a deleted task back; `Some(true)` deletes it, as `delete_task` does.
    pub deleted: Option<bool>,
    /// A note to append to the thread in the same call. Optional in general — and **required** when the
    /// change moves the task into `done`.
    pub comment: Option<String>,
    /// Who the comment is from. Required whenever `comment` is set, because MCP has no session to derive
    /// an author from.
    pub comment_by: Option<String>,
}

impl TaskPatch {
    fn is_empty(&self) -> bool {
        self.text.is_none()
            && self.status.is_none()
            && self.priority.is_none()
            && self.kind.is_none()
            && self.goal.is_none()
            && self.assignee.is_none()
            && self.add_labels.is_empty()
            && self.remove_labels.is_empty()
            && self.depends_on.is_none()
            && self.subtasks.is_empty()
            && self.documents.is_empty()
            && self.gh_actions.is_empty()
            && self.deleted.is_none()
            && self.comment.is_none()
    }

    /// The comment as it should be stored, or `None` when there is nothing to append.
    fn trimmed_comment(&self) -> Option<&str> {
        self.comment
            .as_deref()
            .map(str::trim)
            .filter(|itm| !itm.is_empty())
    }
}

/// Lower-case, trim, drop blanks, de-duplicate, sort.
///
/// Sorting is not cosmetic: it makes a task's label list identical whichever order the tags arrived
/// in, so two boards built by different routes compare equal.
fn normalise_labels(src: &[String]) -> Vec<String> {
    let mut labels: Vec<String> = src
        .iter()
        .map(|itm| itm.trim().to_lowercase())
        .filter(|itm| !itm.is_empty())
        .collect();

    labels.sort();
    labels.dedup();
    labels
}

fn validate_status(project: &ProjectModel, status: &str) -> Result<String, String> {
    let status = status.trim().to_lowercase();

    if project.has_column(&status) {
        return Ok(status);
    }

    Err(format!(
        "'{status}' is not a column of {}; it has: {}",
        project.prefix,
        available_columns(project)
    ))
}

fn available_columns(project: &ProjectModel) -> String {
    let mut ids = vec![COLUMN_ID_TODO.to_string()];
    ids.extend(project.columns.iter().map(|itm| itm.id.clone()));
    ids.push(task_manager_shared::projects::COLUMN_ID_DONE.to_string());
    ids.join(", ")
}

fn validate_kind(project: &ProjectModel, kind: &str) -> Result<Option<String>, String> {
    let kind = kind.trim().to_lowercase();

    // An empty kind is how a caller says "no kind", which is legal — kinds are optional.
    if kind.is_empty() {
        return Ok(None);
    }

    if project.has_kind(&kind) {
        return Ok(Some(kind));
    }

    let known: Vec<&str> = project.kinds.iter().map(|itm| itm.id.as_str()).collect();

    Err(if known.is_empty() {
        format!("{} has no kinds configured", project.prefix)
    } else {
        format!(
            "'{kind}' is not a kind of {}; it has: {}",
            project.prefix,
            known.join(", ")
        )
    })
}

fn parse_dependencies(src: &[String], project: &ProjectModel) -> Result<Vec<i64>, String> {
    let mut numbers = Vec::with_capacity(src.len());

    for entry in src {
        numbers.push(parse_dependency(entry, project)?);
    }

    numbers.sort_unstable();
    numbers.dedup();
    Ok(numbers)
}

/// Everything a caller passes to create a task. A struct rather than six positional arguments — two of
/// them are `Option<String>` and would be trivially swappable.
///
/// There is deliberately **no status**: a new task always starts in Todo. That is not only tidiness — it
/// is what makes the Done rule airtight. If a task could be created straight into Done, "create it there"
/// would be a way around having to say what was done.
pub struct NewTask {
    pub project_prefix: String,
    pub text: String,
    /// How urgent it is. `None` is Normal — most work is, and making every caller rank everything would make
    /// the ranking mean nothing.
    pub priority: Option<String>,
    pub kind: Option<String>,
    /// Which goal this task is part of, by goal id. `None` leaves it standalone.
    pub goal: Option<String>,
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub depends_on: Vec<String>,
    /// The checklist to start it with. Usually empty: a task is broken down once somebody has looked at it,
    /// which is a later call rather than this one.
    pub subtasks: Vec<super::NewSubtask>,
    /// Documents to point at from the start, by id. Every one is checked to be a live document of this same
    /// project before a task number is reserved.
    pub documents: Vec<String>,
}

/// Put a new task on a board. Returns its handle.
pub async fn create_task(app: &AppContext, new_task: NewTask) -> Result<String, String> {
    let board = app.board.read();
    let project = resolve_project_by_prefix(&board, &new_task.project_prefix)?;

    if new_task.text.trim().is_empty() {
        return Err("a task needs some text".to_string());
    }

    // Everything is validated before a number is reserved, so a rejected call leaves the counter
    // exactly where it was and does not burn an id.
    let kind = match &new_task.kind {
        None => None,
        Some(kind) => validate_kind(&project, kind)?,
    };

    let priority = match &new_task.priority {
        None => task_manager_shared::priority::Priority::default(),
        Some(priority) => super::parse_priority(priority)?,
    };

    let depends_on = parse_dependencies(&new_task.depends_on, &project)?;
    let labels = normalise_labels(&new_task.labels);
    let subtasks = super::build_subtasks(&new_task.subtasks)?;

    // Validated against the project rather than accepted blindly: a goal from another board would draw the
    // task under something nobody on this one can see, and a closed goal would gain a live task.
    //
    // Before the reservation, with the rest of the validation — a refused goal used to burn a number,
    // which contradicted the promise made right above.
    let goal_number = resolve_goal(&board, &project, new_task.goal.as_deref())?;

    // Also before the reservation, and the one piece of validation here that reads Postgres: documents are
    // not in memory, so "does this id name a live document of this project" is a query. Refusing after a
    // number was handed out would burn it — the promise made at the top of this function.
    let mut documents = Vec::new();

    super::DocumentsPatch {
        add: new_task.documents.clone(),
        ..Default::default()
    }
    .apply(app, &project.id, &mut documents, "this task")
    .await?;

    let number = app.board.reserve_task_number(&project.id).ok_or_else(|| {
        format!(
            "project {} vanished while creating the task",
            project.prefix
        )
    })?;

    let now = DateTimeAsMicroseconds::now();

    let task = TaskModel {
        project_id: project.id.clone(),
        number,
        text: new_task.text.trim().to_string(),
        // Always Todo. Moving it on is tasks_update's job, which is where the Done rule lives.
        status: COLUMN_ID_TODO.to_string(),
        priority,
        kind,
        goal_number,
        assignee: normalise_assignee(new_task.assignee.as_deref()),
        labels,
        depends_on,
        subtasks,
        documents,
        // Never anything here on creation, and there is deliberately no way to pass one: a build is what
        // comes OUT of the work, so it cannot exist before the task that produced it does. They arrive
        // through tasks_update as the work ships.
        gh_actions: Vec::new(),
        comments: Vec::new(),
        created: now,
        updated: now,
        // Nothing is created closed — a task always starts in Todo.
        close_moment: None,
        deleted_moment: None,
    };

    let ctx = MyTelemetryContext::create_empty();
    let dto: TaskDto = (&task).into();
    app.tasks_repo.upsert(&dto, &ctx).await;

    // The counter moved in memory when the number was reserved; persist it so a restart does not
    // hand the same number out again.
    persist_project_counter(app, &project.id, &ctx).await;

    app.board.upsert_task(task);
    app.notify_project_changed(&project.id).await;

    Ok(crate::board::compose_task_handle(&project.prefix, number))
}

/// Whether a change **lands** work, and therefore owes an explanation.
///
/// The transition, not the destination: a task already in Done can be re-labelled, reassigned or
/// re-worded without being made to justify itself a second time. Only arriving there is the moment worth
/// recording.
///
/// `new_status` is the raw stored value, already validated against the project, so comparing it to the
/// anchor directly is enough — an unknown status can never equal `done`.
fn is_landing(was_done: bool, new_status: &str) -> bool {
    new_status == COLUMN_ID_DONE && !was_done
}

/// Turn the comment half of a patch into something to append, or nothing.
///
/// An author is required as soon as there is any text: MCP has no session to derive one from, so the
/// alternative would be a thread of anonymous notes.
fn build_comment(
    comment: Option<&str>,
    comment_by: Option<&str>,
) -> Result<Option<crate::board::CommentModel>, String> {
    let Some(comment) = comment else {
        return Ok(None);
    };

    let who = comment_by
        .map(str::trim)
        .filter(|itm| !itm.is_empty())
        .ok_or_else(|| {
            "a comment needs an author — pass `comment_by` as an email, or `AI`".to_string()
        })?;

    Ok(Some(crate::board::CommentModel {
        moment: DateTimeAsMicroseconds::now(),
        who: normalise_actor(who),
        text: comment.to_string(),
    }))
}

/// How an email or the reserved `AI` is stored, for an assignee and a comment's author alike.
///
/// One function because the two must not drift: a comment signed `AI` and a task assigned `AI` have to be
/// the same string, or a filter on one silently misses the other.
///
/// Addresses are lower-cased — they are case-insensitive, and storing two spellings of one would split a
/// person in two. `AI` is stored exactly as declared whatever case it arrived in, so the value that comes
/// back is the value the tool descriptions told the caller to use.
pub(super) fn normalise_actor(src: &str) -> String {
    if task_manager_shared::users::is_ai_assignee(src) {
        return task_manager_shared::users::ASSIGNEE_AI.to_string();
    }

    src.to_lowercase()
}

/// The goal number to store, validated against the project the task is on.
///
/// `Ok(None)` for "no goal" — either nothing was passed or an empty string was, which is how a caller
/// detaches a task. Accepts a handle (`RMS-G7`) or a bare number, the same leniency `depends_on` gets: a
/// number is unambiguous within a project, and a caller editing one task should not have to retype the
/// prefix.
///
/// Refused in three cases, each of which would otherwise create something that looks fine:
///
/// * a handle whose prefix belongs to another project — a task can only be part of a goal on its own board;
/// * a number naming no goal — unlike a dependency, where an unknown number keeps the task blocked and is
///   therefore safe, an unknown goal would simply make the task read as standalone and lose the link;
/// * a **closed** goal. This is the second of the three doors into "a closed goal has no live tasks", and
///   the easiest one to walk through by accident: closing a goal checks its tasks, and nothing would stop
///   the very next call from hanging a fresh one underneath.
fn resolve_goal(
    board: &crate::board::BoardInner,
    project: &crate::board::ProjectModel,
    goal: Option<&str>,
) -> Result<Option<i64>, String> {
    let Some(goal) = goal.map(str::trim).filter(|itm| !itm.is_empty()) else {
        return Ok(None);
    };

    // Parsing, project ownership and existence are one question wherever a goal is named, so they live in
    // one place. What is special here is only the last check.
    let number = super::resolve_goal_reference(board, project, goal)?;

    let handle = crate::board::compose_goal_handle(&project.prefix, number);

    let Some(found) = board.get_goal(&project.id, number) else {
        return Err(format!("no goal {handle} on {}", project.prefix));
    };

    if found.is_closed() {
        return Err(format!(
            "{handle} is closed — re-open it before putting work under it, or choose another goal"
        ));
    }

    Ok(Some(number))
}

/// An assignee, normalised by [`normalise_actor`]. Blank means nobody.
fn normalise_assignee(src: Option<&str>) -> Option<String> {
    let src = src?.trim();

    if src.is_empty() {
        return None;
    }

    Some(normalise_actor(src))
}

/// Write the project row purely to save its task counter.
///
/// Reads the project back out of memory first: the copy captured before `reserve_task_number` still
/// has the old counter, and persisting that would undo the reservation.
pub(super) async fn persist_project_counter(app: &AppContext, project_id: &str, ctx: &MyTelemetryContext) {
    if let Some(project) = app.board.read().get_project(project_id) {
        let dto: crate::postgres::ProjectDto = project.as_ref().into();
        app.projects_repo.upsert(&dto, ctx).await;
    }
}

/// Change a task. Returns its handle.
pub async fn update_task(
    app: &AppContext,
    handle: &str,
    patch: TaskPatch,
) -> Result<String, String> {
    if patch.is_empty() {
        return Err(
            "nothing to update: pass at least one of text, status, priority, kind, assignee, labels, dependencies, a checklist change, a document reference, a build link or comment"
                .to_string(),
        );
    }

    let board = app.board.read();
    let resolved = resolve_task(&board, handle)?;
    let project = resolved.project;
    let mut task = resolved.task.as_ref().clone();

    // Read before anything is applied: whether this change *moves* the task into Done is the question, not
    // whether it ends up there. A task already in Done can be re-labelled or reassigned without being made
    // to justify itself again.
    let was_done = project.effective_status(&task.status) == COLUMN_ID_DONE;

    if let Some(text) = &patch.text {
        if text.trim().is_empty() {
            return Err("a task needs some text".to_string());
        }
        task.text = text.trim().to_string();
    }

    if let Some(status) = &patch.status {
        task.status = validate_status(&project, status)?;
    }

    if let Some(priority) = &patch.priority {
        task.priority = super::parse_priority(priority)?;
    }

    if let Some(kind) = &patch.kind {
        task.kind = validate_kind(&project, kind)?;
    }

    if let Some(goal) = &patch.goal {
        task.goal_number = resolve_goal(&board, &project, Some(goal))?;
    }

    // The third door into "a closed goal has no live tasks", and the only one that does not involve the
    // goal at all: the task was done, its goal was closed on the strength of that, and now the task is
    // being re-opened underneath it. Refused rather than silently re-opening the goal, because re-opening
    // an epic is a decision that belongs in its thread, not a side effect of moving one sticker.
    if project.effective_status(&task.status) != COLUMN_ID_DONE {
        if let Some(number) = task.goal_number {
            if let Some(goal) = board.get_goal(&project.id, number) {
                if goal.is_closed() {
                    return Err(format!(
                        "{handle} is part of {}, which is closed — re-open the goal first, or detach the task from it by passing an empty `goal`",
                        crate::board::compose_goal_handle(&project.prefix, number)
                    ));
                }
            }
        }
    }

    if let Some(assignee) = &patch.assignee {
        task.assignee = normalise_assignee(Some(assignee));
    }

    if !patch.add_labels.is_empty() || !patch.remove_labels.is_empty() {
        let mut labels = task.labels.clone();
        labels.extend(normalise_labels(&patch.add_labels));

        let removing = normalise_labels(&patch.remove_labels);
        // Removal after addition, so a label passed to both ends up removed. Arbitrary, but it has to
        // be one or the other and this is the order a reader assumes.
        labels.retain(|itm| !removing.contains(itm));

        task.labels = normalise_labels(&labels);
    }

    if let Some(depends_on) = &patch.depends_on {
        task.depends_on = parse_dependencies(depends_on, &project)?;
    }

    // Applied to the clone, so an op naming an item that is not there refuses the whole call and leaves both
    // Postgres and memory untouched — including the other fields this patch had already set on the clone.
    patch.subtasks.apply(&mut task.subtasks, handle)?;

    // On the clone as well, and for the same reason — but this one reads Postgres to check the ids, because a
    // document is the one thing this service does not hold in memory.
    patch
        .documents
        .apply(app, &project.id, &mut task.documents, handle)
        .await?;

    // On the clone too, and validated the same way — but nothing outside the call is consulted: a build link
    // is a url somebody reports, and this service never goes and looks at it.
    patch
        .gh_actions
        .apply(&mut task.gh_actions, DateTimeAsMicroseconds::now())?;

    let comment = build_comment(patch.trimmed_comment(), patch.comment_by.as_deref())?;
    let landing = is_landing(was_done, &task.status);

    if landing && comment.is_none() {
        return Err(format!(
            "{handle} is being moved to done, so it needs a comment saying what was actually done — pass `comment` (and `comment_by`) with this same call. A line or two: what changed, and anything the next person should know."
        ));
    }

    if let Some(comment) = comment {
        task.comments.push(comment);
    }

    // Stamped on the way in and cleared on the way out, so a re-opened task carries no close date and a
    // re-closed one is dated by its latest close. Left alone while the task simply sits in Done, which is
    // what keeps the archive window measuring "closed N days ago" rather than "last touched N days ago".
    if landing {
        task.close_moment = Some(DateTimeAsMicroseconds::now());
    } else if task.status != COLUMN_ID_DONE {
        task.close_moment = None;
    }

    // Stamped once and cleared whole — see the same block on a goal.
    match patch.deleted {
        Some(true) => {
            if task.deleted_moment.is_none() {
                task.deleted_moment = Some(DateTimeAsMicroseconds::now());
            }
        }
        Some(false) => task.deleted_moment = None,
        None => {}
    }

    task.updated = DateTimeAsMicroseconds::now();

    let ctx = MyTelemetryContext::create_empty();
    let dto: TaskDto = (&task).into();
    app.tasks_repo.upsert(&dto, &ctx).await;

    let handle = crate::board::compose_task_handle(&project.prefix, task.number);
    app.board.upsert_task(task);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// Mark a task deleted.
///
/// **It is not removed — from Postgres or from memory — and that is the change this replaced a hard delete
/// with.** A deleted task drops out of every list, every count and every derived answer, and stays exactly
/// where it was for one purpose: somebody searching for its id finds it and is told it is gone. A row that
/// had been deleted outright could only answer "no such task", which is indistinguishable from a typo and
/// from an id that belongs to another board.
///
/// Deleting twice is not an error and does not move the moment: the caller's intent is already true, and
/// re-stamping it would rewrite when it happened.
pub async fn delete_task(app: &AppContext, handle: &str) -> Result<String, String> {
    let board = app.board.read();
    let resolved = resolve_task(&board, handle)?;
    let project = resolved.project;
    let mut task = resolved.task.as_ref().clone();

    if task.deleted_moment.is_none() {
        task.deleted_moment = Some(DateTimeAsMicroseconds::now());
        task.updated = DateTimeAsMicroseconds::now();
    }

    let ctx = MyTelemetryContext::create_empty();
    let dto: TaskDto = (&task).into();
    app.tasks_repo.upsert(&dto, &ctx).await;

    let handle = crate::board::compose_task_handle(&project.prefix, task.number);
    app.board.upsert_task(task);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

/// Append a comment to a task's thread.
///
/// Does **not** move the task's `updated`: the thread is a record of the conversation about the work,
/// not a change to the work.
pub async fn add_comment(
    app: &AppContext,
    handle: &str,
    who: &str,
    text: &str,
) -> Result<String, String> {
    if who.trim().is_empty() {
        return Err("a comment needs an author — an email, or `AI`".to_string());
    }

    if text.trim().is_empty() {
        return Err("a comment needs some text".to_string());
    }

    let board = app.board.read();
    let resolved = resolve_task(&board, handle)?;
    let project = resolved.project;
    let mut task = resolved.task.as_ref().clone();

    task.comments.push(crate::board::CommentModel {
        moment: DateTimeAsMicroseconds::now(),
        who: who.trim().to_lowercase(),
        text: text.trim().to_string(),
    });

    let ctx = MyTelemetryContext::create_empty();
    let dto: TaskDto = (&task).into();
    app.tasks_repo.upsert(&dto, &ctx).await;

    let handle = crate::board::compose_task_handle(&project.prefix, task.number);
    app.board.upsert_task(task);
    app.notify_project_changed(&project.id).await;

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rule is about the transition, not the destination. Getting this wrong in either direction is
    /// what makes the feature annoying: demand a comment on every edit of a finished task, or let work
    /// land silently.
    #[test]
    fn only_arriving_at_done_owes_an_explanation() {
        assert!(is_landing(false, COLUMN_ID_DONE), "todo -> done lands work");

        assert!(
            !is_landing(true, COLUMN_ID_DONE),
            "already done and staying done is not a landing"
        );
        assert!(
            !is_landing(false, COLUMN_ID_TODO),
            "moving between other columns is not a landing"
        );
        assert!(
            !is_landing(true, COLUMN_ID_TODO),
            "re-opening a finished task is not a landing"
        );
    }

    #[test]
    fn no_comment_text_means_nothing_to_append() {
        assert!(build_comment(None, None).unwrap().is_none());
        assert!(build_comment(None, Some("yuri@mxtm.ai")).unwrap().is_none());
    }

    /// A thread of anonymous notes would be worthless, and MCP has no session to fall back on — so text
    /// without an author is refused rather than stored as "somebody".
    #[test]
    fn a_comment_without_an_author_is_refused() {
        for author in [None, Some(""), Some("   ")] {
            assert!(
                build_comment(Some("did the thing"), author).is_err(),
                "{author:?} should not be accepted as an author"
            );
        }
    }

    #[test]
    fn an_author_is_stored_lower_cased() {
        let comment = build_comment(Some("did the thing"), Some("  Yuri@MXTM.ai "))
            .unwrap()
            .expect("should build");

        assert_eq!(comment.who, "yuri@mxtm.ai");
        assert_eq!(comment.text, "did the thing");
    }

    /// `AI` is a legitimate author and must survive untouched — it is how an agent signs its own note.
    #[test]
    fn ai_is_a_legitimate_author() {
        let comment = build_comment(Some("built it"), Some("AI"))
            .unwrap()
            .expect("should build");

        assert_eq!(comment.who, "AI");

        // Any case reads as the same reserved value, so a filter on one spelling finds the other.
        let lower = build_comment(Some("built it"), Some(" ai "))
            .unwrap()
            .expect("should build");

        assert_eq!(lower.who, "AI");
    }

    /// The two paths must agree, or a comment signed `AI` and a task assigned `AI` would be different
    /// strings and a filter on one would miss the other.
    #[test]
    fn an_assignee_and_an_author_normalise_the_same_way() {
        assert_eq!(normalise_assignee(Some("AI")).as_deref(), Some("AI"));
        assert_eq!(normalise_assignee(Some("ai")).as_deref(), Some("AI"));
        assert_eq!(
            normalise_assignee(Some(" Yuri@MXTM.ai ")).as_deref(),
            Some("yuri@mxtm.ai")
        );
        assert_eq!(normalise_assignee(Some("   ")), None);
        assert_eq!(normalise_assignee(None), None);
    }

    /// Whitespace-only text is not a comment. Without this, a caller could satisfy the Done rule with a
    /// single space and record nothing — which is exactly what the rule exists to prevent.
    #[test]
    fn blank_comment_text_does_not_count() {
        let patch = TaskPatch {
            comment: Some("   \n  ".to_string()),
            ..Default::default()
        };

        assert!(patch.trimmed_comment().is_none());
    }

    /// A patch carrying only a comment is a real change — it appends to the thread — so it must not be
    /// rejected as an empty update.
    #[test]
    fn a_comment_alone_is_not_an_empty_update() {
        let patch = TaskPatch {
            comment: Some("a finding".to_string()),
            ..Default::default()
        };

        assert!(!patch.is_empty());
        assert!(TaskPatch::default().is_empty());
    }

    /// Ticking one checklist item is the smallest real change there is, and it arrives with no other field
    /// set — so the emptiness check has to see it, or the one call this feature exists for is refused.
    #[test]
    fn a_checklist_change_alone_is_not_an_empty_update() {
        let patch = TaskPatch {
            subtasks: crate::scripts::SubtasksPatch {
                check: vec!["some-id".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }

    /// Attaching a document arrives with nothing else set, exactly as ticking a checklist item does — so the
    /// emptiness check has to see it, or the only call that makes a reference is refused as "nothing to
    /// update".
    #[test]
    fn a_document_reference_alone_is_not_an_empty_update() {
        let patch = TaskPatch {
            documents: crate::scripts::DocumentsPatch {
                add: vec!["some-id".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }

    /// And so does a build link, which is the whole of the call an agent makes after shipping something —
    /// nothing else about the task changed, so the emptiness check has to see it.
    #[test]
    fn a_build_link_alone_is_not_an_empty_update() {
        let patch = TaskPatch {
            gh_actions: crate::scripts::GhActionsPatch {
                add: vec![crate::scripts::NewGhAction {
                    url: "https://github.com/o/r/actions/runs/1".to_string(),
                    title: None,
                }],
                ..Default::default()
            },
            ..Default::default()
        };

        assert!(!patch.is_empty());
    }
}
