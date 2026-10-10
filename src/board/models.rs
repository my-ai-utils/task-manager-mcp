use std::collections::BTreeSet;

use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::priority::Priority;

/// One column of a project's board, in memory.
///
/// The two anchors (`todo`, `done`) are not represented here — they exist by definition, and a
/// reader adds them at either end. `order` places this column between them.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub order: i32,
}

/// A named set of columns, shared by any number of projects.
///
/// Columns live here rather than on a project. `columns` is kept sorted by `order` on write, so every
/// read is already in board order.
#[derive(Debug, Clone)]
pub struct ColumnTemplateModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub columns: Vec<ColumnModel>,
    pub created: DateTimeAsMicroseconds,
}

/// A named set of task types, shared by any number of projects.
///
/// Same shape and same reason as [`ColumnTemplateModel`]: the vocabulary of work is a property of how a
/// team works, not of one project, so it is defined once and followed.
#[derive(Debug, Clone)]
pub struct KindTemplateModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kinds: Vec<KindModel>,
    pub created: DateTimeAsMicroseconds,
}

/// One kind of work, in memory. Unlike a column id, a kind is optional on a task.
#[derive(Debug, Clone, PartialEq)]
pub struct KindModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: KindColor,
    // An icon name without extension, or empty for none. Not validated against a list: the icons are
    // files in the UI bundle and the server has no business knowing which ones shipped — an unknown name
    // draws as no icon, the same leniency a colour gets.
    pub icon: String,
}

/// One connected GitHub repository, in memory.
///
/// **The connection, not the mirror.** This is the four things a person configured — what to call it,
/// which repository, which branch, which folder — and it is the whole of what survives a restart. What
/// was actually pulled, when, and whether a key is held for it live in `crate::github::GithubMirrors`,
/// which is rebuilt from nothing every time the process starts.
///
/// The split is why this is on the board at all: the board is the configured shape of the product, and
/// a repository somebody attached to a project is exactly that. A downloaded tree is not.
#[derive(Debug, Clone, PartialEq)]
pub struct GithubConnectionModel {
    // What it is called, and the folder it appears as: `github/<name>/…`. One path segment, unique
    // within the project.
    pub name: String,
    pub owner: String,
    pub repo: String,
    // Empty for the repository's default branch — resolved at pull time, because which branch is default
    // is the repository's business and can change without anybody here being told.
    pub branch: String,
    // Which folder inside the repository is mirrored, or empty for the whole of it.
    pub repo_path: String,
}

impl GithubConnectionModel {
    /// `owner/repo`, which is how a repository is named everywhere a person reads one.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
}

/// A project, in memory.
///
/// `members` is a `BTreeSet` rather than a `Vec`: membership is a set with no meaningful order,
/// asked about far more often than it is edited ("may this person see this board?"), and the sorted
/// iteration makes a listing reproducible.
#[derive(Debug, Clone)]
pub struct ProjectModel {
    pub id: String,
    pub name: String,
    pub description: String,
    // Stored upper-cased, which is also how a parsed handle arrives, so a lookup needs no
    // normalisation at the call site.
    pub prefix: String,
    // Every prefix this project has carried before the current one, upper-cased, oldest first.
    pub prefix_history: Vec<String>,
    // Which column template this project follows, and `None` for a project that follows none — whose
    // board is then just Todo → Done. Not an error: creating a project would otherwise require creating
    // a template first.
    pub column_template_id: Option<String>,
    // The columns resolved from that template, sorted by `order`.
    //
    // A CACHE, not the source of truth: `rebuild_indexes` recomputes it from the templates on every
    // write, so it cannot drift — editing a template updates every project that follows it in the same
    // swap. It exists because `has_column`, `effective_status`, the board read and the MCP tools all ask
    // a project for its columns, and threading the template collection through every one of them would
    // spread this indirection across the whole service instead of confining it to one function.
    pub columns: Vec<ColumnModel>,
    // Which task-type template this project follows, and the types resolved from it. Exactly the same
    // arrangement as the columns above, cache and all — see the comment there for why.
    pub kind_template_id: Option<String>,
    pub kinds: Vec<KindModel>,
    pub members: BTreeSet<String>,
    // High-water mark of the project's own counter, shared by tasks, goals AND releases: a number is
    // handed out once and names exactly one of them. Only ever moves forward — deleting a task does not
    // lower it, which is what keeps a number from being handed out twice.
    pub last_task_number: i64,
    // How long finished work stays on the board before it counts as archived. `None` means the default
    // seven days, which is what every project did before this was configurable — so a row that has never
    // been told otherwise keeps behaving exactly as it did.
    pub archive_days: Option<i32>,
    // When this project was archived — put away, not deleted — and `None` for one that is not.
    //
    // Archiving takes a project out of every picker and out of NOTHING else. It still answers
    // `get_project_by_prefix`, its tasks and documents still open by direct link, it still follows its
    // templates, and it still holds its prefix against a new project taking it. That last one is the cost
    // worth naming out loud: archiving does not free `RMS` for reuse, because if it did, the archived
    // board's links would silently start landing on somebody else's.
    //
    // Not a second reading of `archive_days` above — that one is a finished TASK ageing off the board, and
    // the two have nothing to do with each other.
    pub archived_moment: Option<DateTimeAsMicroseconds>,
    // The GitHub repositories mirrored into this project's documents. Ordered as they were configured
    // and unique by `name`, which is enforced on the write path — the name is a folder, and two folders
    // with one name is a tree nobody can read.
    pub github_connections: Vec<GithubConnectionModel>,
    pub created: DateTimeAsMicroseconds,
}

impl ProjectModel {
    /// One connection by name, or `None`. Case-sensitive, like every path in this product.
    pub fn github_connection(&self, name: &str) -> Option<&GithubConnectionModel> {
        self.github_connections.iter().find(|itm| itm.name == name)
    }

    /// Whether `column_id` is a column this project actually has, counting the two anchors.
    pub fn has_column(&self, column_id: &str) -> bool {
        task_manager_shared::projects::is_anchor_column(column_id)
            || self.columns.iter().any(|itm| itm.id == column_id)
    }

    /// Whether `kind_id` is a kind this project actually has.
    pub fn has_kind(&self, kind_id: &str) -> bool {
        self.kinds.iter().any(|itm| itm.id == kind_id)
    }

    /// The status a task should be *read* as.
    ///
    /// A stored status naming a column this project no longer has reads as Todo. The stored value is
    /// left alone, so re-creating the column brings the task back to it — which is why this is a
    /// read-side function and not a migration.
    pub fn effective_status(&self, stored: &str) -> String {
        if self.has_column(stored) {
            stored.to_string()
        } else {
            task_manager_shared::projects::COLUMN_ID_TODO.to_string()
        }
    }

    /// The kind a task should be read as: `None` when it has none, and also when it names a kind
    /// this project no longer has. Same leniency as a status, same reason.
    pub fn effective_kind(&self, stored: Option<&str>) -> Option<String> {
        let stored = stored?;

        if self.has_kind(stored) {
            Some(stored.to_string())
        } else {
            None
        }
    }

    /// Whether this email may see the project. Admins bypass this entirely — they are not members.
    pub fn is_member(&self, email: &str) -> bool {
        self.members.contains(&email.trim().to_lowercase())
    }

    /// Whether this project has been put away.
    ///
    /// **Read the receiver.** `BoardInner::is_archived(&TaskModel)`, a few hundred lines away in the same
    /// crate, is the finished-task window and answers an entirely different question. The two share a word
    /// and nothing else.
    pub fn is_archived(&self) -> bool {
        self.archived_moment.is_some()
    }

    /// How long finished work stays on this board before it counts as archived.
    ///
    /// `None` means the default, and so does a number that makes no sense: a stored zero or a negative
    /// would archive everything the instant it closed, which is not a setting anybody means to make. The
    /// leniency is on the read side on purpose — writes validate — so one bad row cannot empty a board.
    pub fn archive_after(&self) -> std::time::Duration {
        match self.archive_days {
            Some(days) if days > 0 => {
                std::time::Duration::from_secs(days as u64 * 24 * 60 * 60)
            }
            _ => super::ARCHIVE_AFTER,
        }
    }
}

/// A goal, in memory — the container work is done around. An epic.
///
/// Identified by `(project_id, number)`, exactly like a task, and out of the same per-project counter: a
/// number names either a task or a goal and never both, so `RMS-7` and `RMS-G7` cannot coexist. The
/// handle is composed on read from the project's current prefix and is not stored, for the same reason a
/// task's is not — a prefix moves between projects.
///
/// It does not list its tasks: a task carries the goal's number. That direction is also the one every
/// read wants, since a task is drawn far more often than a goal is listed.
///
/// **There is no `status` field, and that is the point.** A goal has exactly two states in this version,
/// and `close_moment` already tells them apart — storing a status beside it would let the two disagree
/// (`done` with no moment reads as never closed; a moment with `todo` reads as archived while open), and
/// a goal that is closed according to one field and open according to the other is a bug nobody sees
/// until a screen goes blank. The wire still reports `status`; it is derived. When the second iteration
/// gives a goal real columns, the field arrives then and pays for itself.
#[derive(Debug, Clone)]
pub struct GoalModel {
    pub project_id: String,
    pub number: i64,
    pub name: String,
    pub description: String,
    // Visual only, and from the same palette task types use: a goal is recognised on a board by its
    // colour before anybody reads the strip, and a second palette would break that at a glance.
    pub color: KindColor,
    // How urgent the goal is — the same scale a task carries, and what decides where it sits on the Goals
    // screen. Not derived from the tasks under it: an epic can be urgent while its first task is not, and a
    // number computed from the work would take the decision away from the person making it.
    pub priority: Priority,
    // The goal's own checklist, in the order it was written. Private to the goal: it says nothing about
    // whether the goal can close — that is decided by its TASKS, and folding the two together would make
    // one counter mean two things.
    pub subtasks: Vec<SubtaskModel>,
    // Ids of the documents this goal references, oldest first. See `TaskModel::documents` — the same list
    // for the same reasons, and a goal is the likelier of the two to point at a written-down decision.
    pub documents: Vec<String>,
    // Numbers of the releases this goal went out in, in the order they were attached. Numbers and not
    // handles for the reason `TaskModel::depends_on` holds numbers: a release is on this same project, so
    // the prefix is implied and storing it would only give it a way to go stale.
    //
    // **The link lives here, on the goal** — a release is a project-level record that knows nothing about
    // goals, and a goal lists what it shipped in. Read it through `BoardInner::releases_of_goal`: a number
    // naming a release that has been deleted is skipped rather than reported, and the stored value is left
    // alone, so undeleting the release brings it back onto the goal.
    //
    // Nothing stops one release being listed by two goals. It rarely means anything — a release is one
    // feature going out, and the goal is that feature — but it is not this list's business to forbid.
    pub releases: Vec<i64>,
    // The discussion the work came out of. Same shape as a task's thread and the same reason it rides on
    // the row: one atomic write per comment.
    pub comments: Vec<CommentModel>,
    pub created: DateTimeAsMicroseconds,
    // Moved by a change to the goal itself. A comment does NOT move it, as on a task.
    pub updated: DateTimeAsMicroseconds,
    // When work on the goal began, and `None` until it has. The START of the goal's span on the timeline —
    // which `created` is not: a goal is opened while it is still being talked about, and the work can start
    // weeks later.
    //
    // Set by whoever does the work, through goals_create / goals_update; and if nobody has, stamped by the
    // server when the first task under the goal leaves Todo — work has begun by then whether anybody said
    // so or not. Never cleared by anything but an explicit call: re-opening a goal does not un-start it.
    pub start_moment: Option<DateTimeAsMicroseconds>,
    // When the goal was closed, and `None` while it is open — which makes this the whole of its state.
    // Cleared on re-opening, so a re-closed goal is dated by its latest close. The END of its span on the
    // timeline.
    pub close_moment: Option<DateTimeAsMicroseconds>,
    // When it was deleted, and `None` for one that is not.
    //
    // **Deleted is not a third state beside open and closed** — it is orthogonal to both. A goal is deleted
    // because it should never have existed, which is a different statement from how it went; closing is the
    // statement about how it went, and it demands a resolution for exactly that reason.
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

impl GoalModel {
    /// Whether the goal is closed. The only state question there is — see the note on the struct.
    pub fn is_closed(&self) -> bool {
        self.close_moment.is_some()
    }

    /// Whether it has been deleted. Orthogonal to closed: a deleted goal may have been either.
    pub fn is_deleted(&self) -> bool {
        self.deleted_moment.is_some()
    }

    /// The status a reader shows: `done` for a closed goal, `todo` for an open one. Derived rather than
    /// stored so it cannot disagree with `close_moment`.
    pub fn status(&self) -> &'static str {
        if self.is_closed() {
            task_manager_shared::projects::COLUMN_ID_DONE
        } else {
            task_manager_shared::projects::COLUMN_ID_TODO
        }
    }
}

/// One microservice inside a release: which version of it went out, and built from which commit.
///
/// **`microservice_id` is the identity within its release.** A release names a service once, so reporting
/// the same one again corrects the entry rather than adding a second — see `ServicesPatch`.
///
/// `settings_update_note` is separate from `description` on purpose, and the separation is the whole reason
/// it exists: whether a service's settings have to change is the one fact about a release somebody ACTS on
/// while rolling it out, and a fact that has to be found in prose gets missed. Empty means the settings do
/// not change. `description` is everything else worth saying about this service's part of the release.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceReleaseModel {
    pub microservice_id: String,
    pub version: String,
    // The commit this version was built from, lower-cased hex. The half of the entry that outlives a
    // version number: a tag can be moved, and this cannot.
    pub git_hash: String,
    // Where the build of this version can be looked at — the GitHub release, or the run of the workflow
    // that built the image. A link and nothing more: nothing here talks to GitHub, so this is what
    // whoever recorded the release pasted. Empty when there is none, which is the ordinary state of a
    // service that was built and rolled out by hand.
    pub release_link: String,
    // When this service went out. Given by whoever records the release — the one moment in this file that
    // is a statement by the caller rather than a stamp put on by the server, because the server was not
    // there when it happened.
    pub datetime: DateTimeAsMicroseconds,
    pub settings_update_note: String,
    pub description: String,
}

/// A release, in memory — the record that a feature went out, and in what.
///
/// **A project-level thing, not a part of a goal.** It is identified by `(project_id, number)` exactly as a
/// task and a goal are, out of the same per-project counter, so `RMS-7`, `RMS-G7` and `RMS-R7` cannot
/// coexist and a bare number still names one thing. The handle `RMS-R7` is composed on read from the
/// project's current prefix and is not stored, for the reason a task's is not.
///
/// It does not know which goal it shipped: the goal lists its releases (`GoalModel::releases`). That keeps
/// this a plain record of what went out — a release recorded before anybody decided which goal it belongs
/// under is still a release.
///
/// One release is one feature across however many microservices it touched, which is why `services` is a
/// list and the notes are on the release: the release says what changed for a reader, each service says
/// what was deployed.
///
/// There is next to no state machine. A release has happened by the time it is written down, and three
/// things can become of it afterwards: it reaches another environment, which is a label on it rather than
/// a second release — see `envs`; it is closed, once it has reached every one it is going to — see
/// `done_moment`; or it is deleted, for one recorded by mistake.
#[derive(Debug, Clone)]
pub struct ReleaseModel {
    pub project_id: String,
    pub number: i64,
    pub title: String,
    // What this release is, as Markdown — the context around it.
    pub description: String,
    // What changed for whoever is on the receiving end, as Markdown. Separate from `description` because
    // the two are read by different people: one explains the release, the other is what gets pasted to
    // the people affected by it.
    pub release_notes: String,
    // The date of the release, as its author gave it — and what every list of releases is ordered by.
    // Not `created`: a release is often written down after the fact, and the order that matters is the
    // order things went out in.
    pub date: DateTimeAsMicroseconds,
    // In the order they were added, which is normally the order they went out.
    pub services: Vec<ServiceReleaseModel>,
    // The environments this release is out on, as labels — `Dev`, `Prod` — in the order it reached them.
    //
    // A release is written down when it ships SOMEWHERE — usually a test stand first — and reaching the
    // next environment is a later fact about the same versions of the same services. So it is a label on
    // the release rather than a second release, and it is what "what is actually on prod" is filtered by.
    //
    // Labels and not a flag per environment, because which environments there are is the project's own
    // business: one has Dev and Prod, another a stand per client. Nothing here knows the list; a label
    // exists for as long as some release carries it. Each is one word, kept as it was first spelled on
    // the project, and no release carries the same one twice however it is cased — see
    // `EnvsPatch::apply`. Taking a label off is how a release pulled back from an environment stops
    // reading as there.
    pub envs: Vec<String>,
    // When the release was CLOSED, and `None` while it is still going out.
    //
    // Closing says the rollout is over: the release has reached every environment it is going to and
    // nothing more is expected to happen to it. It is what separates the releases somebody still has to
    // do something about from the history under them.
    //
    // **A statement, not something derived from `envs`.** Nothing here knows which environments a project
    // has — a label exists only for as long as some release carries it — so "on all of them" cannot be
    // computed, only said by whoever rolled it out. For the same reason nothing is enforced the other
    // way: a closed release can still be put on an environment or taken off one. It is a mark, not a lock.
    //
    // A moment and not a bool, the way `close_moment` on a goal and `deleted_moment` are: "is it done" and
    // "since when" are one fact, and two fields for it could disagree. Stamped when the release is
    // closed, kept if it is closed again, and cleared whole when it is reopened.
    pub done_moment: Option<DateTimeAsMicroseconds>,
    // What was said about the release: how the rollout went, what was noticed afterwards, why it was
    // pulled back. The same shape as a goal's thread and a task's, and it rides on the row for the same
    // reason — one atomic write per comment. The notes above say what CHANGED; this says what HAPPENED.
    pub comments: Vec<CommentModel>,
    pub created: DateTimeAsMicroseconds,
    // Moved by a change to the release itself. A comment does NOT move it, as on a task and a goal.
    pub updated: DateTimeAsMicroseconds,
    // When it was deleted, and `None` for one that is not. A flag rather than a removal, for the reason a
    // task's is: an id that comes back as "no such release" is indistinguishable from a typo.
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

impl ReleaseModel {
    pub fn is_deleted(&self) -> bool {
        self.deleted_moment.is_some()
    }

    /// Whether it has been closed. Derived, so it cannot disagree with the moment.
    pub fn is_done(&self) -> bool {
        self.done_moment.is_some()
    }

    /// Whether it is out on an environment, by that environment's label — in any case.
    pub fn is_on_env(&self, env: &str) -> bool {
        self.envs
            .iter()
            .any(|itm| task_manager_shared::releases::same_env(itm, env))
    }

    /// What a release marked "on production" by 0.2.0 reads as now: the label `Prod`.
    ///
    /// That build kept one fact about where a release was out — a moment in `released_on_prod_moment` —
    /// and environments replaced it before much had been recorded. The two places old data can still
    /// arrive from, a row nobody has rewritten since and an archive exported back then, both come through
    /// here, so a release that was live does not turn up as out nowhere.
    pub fn envs_of_prod_mark(on_prod: bool) -> Vec<String> {
        if on_prod {
            vec![LEGACY_PROD_ENV.to_string()]
        } else {
            Vec::new()
        }
    }
}

/// The label a 0.2.0 production mark becomes — see [`ReleaseModel::envs_of_prod_mark`].
const LEGACY_PROD_ENV: &str = "Prod";

/// One checklist item, in memory. Carried identically by a task and by a goal.
///
/// **Not a task.** It has no number from the project's counter, no status, no assignee and no thread; it is
/// never drawn on a board and nothing derived reads it — an unticked item does not block a task from
/// landing and does not count towards a goal's progress. It is the breakdown one person wrote to keep
/// track of one piece of work. Work somebody else has to see, schedule or depend on is a task under a
/// goal, and that distinction is the whole reason this type is this small.
///
/// `id` is a `SortableId`, minted when the item is created and never shown to a person: an agent names the
/// item it means to tick, and the screen draws `title`. It is not reused after a removal, so an id in a
/// stale message names nothing rather than the wrong item.
#[derive(Debug, Clone, PartialEq)]
pub struct SubtaskModel {
    pub id: String,
    pub title: String,
    /// The longer half, as Markdown. Empty is normal — a one-line item has nothing to expand.
    pub text: String,
    pub done: bool,
}

/// One build that came out of a task — a GitHub Actions run.
///
/// **The url is the identity.** A run already has an id and it is in the url, so there is nothing to mint
/// here and nothing to look one up by: adding a link that is already on the task does not duplicate it, and
/// removing one names the url.
///
/// `title` is what the card draws and is never empty — one is worked out from the url when the caller gives
/// none, because a naked run url is not something a reader can scan.
///
/// `moment` is when the link was attached, not when GitHub ran the build: nothing here talks to GitHub, and
/// what this side knows is when the work recorded its build.
#[derive(Debug, Clone, PartialEq)]
pub struct GhActionModel {
    pub url: String,
    pub title: String,
    pub moment: DateTimeAsMicroseconds,
}

/// One comment on a task's thread.
#[derive(Debug, Clone)]
pub struct CommentModel {
    pub moment: DateTimeAsMicroseconds,
    pub who: String,
    pub text: String,
}

/// A task, in memory.
///
/// Identified by `(project_id, number)`. The handle a person sees is composed from the project's
/// *current* prefix on every read and is not stored — see the README on why. `depends_on` holds
/// numbers for the same reason.
#[derive(Debug, Clone)]
pub struct TaskModel {
    pub project_id: String,
    pub number: i64,
    pub text: String,
    // The stored status. Read it through `ProjectModel::effective_status` — this field can name a
    // column that no longer exists.
    pub status: String,
    // How urgent it is. Unlike a status or a kind there is nothing to be lenient about: the scale is a
    // product-wide enum rather than per-project configuration, so it cannot name something a project no
    // longer has, and a value nobody set is Normal.
    pub priority: Priority,
    pub kind: Option<String>,
    // The goal this task is part of, by the goal's number within this same project — a task can only
    // belong to a goal on its own board, so the project is implied and storing a prefix would only give
    // it a way to go stale.
    //
    // Read it through `BoardInner::effective_goal`: nothing deletes a goal in this version, but a number
    // naming no goal still reads as "standalone" rather than as an error, the same leniency a status gets.
    pub goal_number: Option<i64>,
    pub assignee: Option<String>,
    // Lower-cased and de-duplicated on write, sorted so a listing is reproducible.
    pub labels: Vec<String>,
    pub depends_on: Vec<i64>,
    // The checklist, in the order it was written. Deliberately outside everything derived: `blocked` reads
    // `depends_on` and nothing else, and moving a task to Done reads its status and nothing else. A
    // checklist somebody abandoned half-way is therefore not a state the board has to have an opinion on.
    pub subtasks: Vec<SubtaskModel>,
    // Ids of the documents this task references — `SortableId`s, sorted, which for a sortable id is also
    // oldest first.
    //
    // **The only part of a document that is in memory.** The documents themselves live in Postgres and are
    // read on request; these ids ride along in the board snapshot so a card can say how many documents a
    // task has without a request, and the texts are fetched only when somebody opens one.
    //
    // An id here may name a document that has since been deleted — nothing prunes the list, because a
    // deletion is undoable and a reference silently dropped would not come back. A reader that cannot
    // resolve one says so.
    pub documents: Vec<String>,
    // The builds this task produced, in the order they were attached — which is also oldest first, since the
    // moment is stamped as each one arrives.
    //
    // The other direction of a document reference, and worth saying out loud because they sit next to each
    // other on the screen: a document is what the work was done AGAINST, a build is what came OUT of it. This
    // one is held whole rather than as an id — there is nothing to resolve, the entry is a link and its name.
    pub gh_actions: Vec<GhActionModel>,
    pub comments: Vec<CommentModel>,
    pub created: DateTimeAsMicroseconds,
    // Moved by a change to the task itself. A comment does NOT move it: the thread is a separate
    // record from the work.
    pub updated: DateTimeAsMicroseconds,
    // When the task left Todo — the moment work on it began — and `None` while it is in Todo.
    //
    // Stamped on the way out of Todo and cleared on the way back, the way `close_moment` is stamped on the
    // way into Done and cleared on the way out: a task put back in the queue has not started, and one taken
    // up again is dated by its latest start. A task moved straight from Todo to Done started and finished
    // at the same moment, which is what the board knows about it.
    pub start_moment: Option<DateTimeAsMicroseconds>,
    // When the task was moved into Done, and `None` whenever it is not there.
    //
    // Separate from `updated` because that moves on every edit, including edits made after the work
    // landed — so it cannot answer "how long ago was this closed", which is what decides whether the task
    // still appears on the board or has aged out into the archive.
    //
    // Cleared when a task is re-opened, so a re-closed task is dated by its latest close rather than its
    // first.
    pub close_moment: Option<DateTimeAsMicroseconds>,
    // When it was deleted, and `None` for one that is not.
    //
    // **A deleted task stays on the board in memory and stays in Postgres**, which is the whole point of the
    // flag: it is hidden from every list and every count, and it is still there when somebody searches for
    // its id — which is the one moment anybody wants a deleted task, and the moment a hard delete had nothing
    // to say.
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

impl TaskModel {
    pub fn is_deleted(&self) -> bool {
        self.deleted_moment.is_some()
    }
}

/// One person.
#[derive(Debug, Clone)]
pub struct UserModel {
    // Lower-cased. The identity, and the primary key in Postgres.
    pub email: String,
    pub name: String,
    // Only half of "is this person an admin" — the settings admin list is additive on top.
    pub admin: bool,
    pub disabled: bool,
    pub created: DateTimeAsMicroseconds,
}
