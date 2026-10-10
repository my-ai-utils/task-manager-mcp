use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.
//
// There is ONE task input model here that writes, and it is deliberately the narrowest possible: a move
// between columns. Everything else about a task — its text, its type, its assignee, its thread — arrives
// through `/mcp`, and the REST surface otherwise reads the board and configures the product. If you find
// yourself adding `CreateTaskInputModel` here, the design changed — update README.md first.

// One comment on a task's thread.
//
// `who` is a user's email or the literal `AI`. It is a plain string rather than a reference to
// the roster: MCP has no session to derive an author from, so it passes one, and an author whose
// user row was removed still has to render.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct TaskCommentResponse {
    pub moment_unix_seconds: i64,
    pub who: String,
    pub text: String,
}

// One build that came out of this task — a GitHub Actions run, as the card links to it.
//
// A bare URL was the other option and is not enough: `https://github.com/org/repo/actions/runs/1842…`
// drawn on a card tells a reader nothing they can scan, so the entry carries what to call it. `title` is
// never empty — one is worked out from the URL when the caller gives none.
//
// `moment` is when the link was ATTACHED, stamped by the server, not when GitHub ran the build. Nothing in
// this service talks to GitHub, and the moment the work recorded its build is the fact this side actually
// knows; a time read off a URL nobody fetched would be a guess dressed as a record.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct TaskGhActionResponse {
    pub url: String,
    pub title: String,
    pub moment_unix_seconds: i64,
}

// A task named by another task, and what it is doing.
//
// A handle on its own is not enough for a reader looking at a dependency: the question a dependency
// raises is "is that done yet", and the answer is the whole reason it is on screen. The status is
// `effective_status` — the same leniency a task's own status gets, so a blocker sitting in a column
// its project no longer has reads as Todo rather than as a column nobody recognises.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct TaskLinkResponse {
    pub id: String,
    pub status: String,
}

// One sticker, as Home draws it.
//
// `status` and `kind` are raw ids, not enums: both vocabularies are configured per project at
// runtime, so a value this build has never heard of must render rather than fail the read. An
// unknown `status` is drawn in Todo; an unknown or absent `kind` is drawn without a kind.
//
// `blocked` and `blocks` are derived server-side against the whole project and are never stored.
// `blocked` is true while any id in `depends_on` names a task that is not Done — including an id
// that matches no task at all, so a typo or a deleted blocker keeps the task blocked instead of
// quietly freeing it. `blocks` is the reverse edge: who is waiting on this one.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct TaskResponse {
    pub id: String,
    // Which board this is on, by PREFIX — the same string the id above carries in front of its number, and
    // the only name a project has on this wire. See `crate::projects::ProjectResponse`.
    pub project: String,
    pub text: String,
    pub status: String,
    // How urgent it is — `task_manager_shared::priority::Priority` on the wire, read back with
    // `parse_or_default`. Never absent: a task nobody has ranked reads as `normal`, and so does a row written
    // before the field existed. It is what decides where the card sits in its column.
    #[serde(default)]
    pub priority: String,
    pub kind: Option<String>,
    // Which goal this task is part of — its handle, `RMS-G7` — and its name. Both absent for a standalone
    // task, which is a normal state and not an unfinished one. Also both absent if the stored number names
    // no goal, which reads the same way on purpose: such a task is standalone rather than dangling.
    pub goal: Option<String>,
    pub goal_name: Option<String>,
    // The goal's palette colour, so the board can mark the card with it without joining a goal list. Absent
    // exactly when `goal` is.
    pub goal_color: Option<String>,
    pub assignee: Option<String>,
    // The assignee's display name, resolved from the roster. Absent when the assignee is `claude`
    // or an email with no user row — Home then shows the raw assignee value.
    pub assignee_name: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub blocks: Vec<String>,
    // The status of every task named in `depends_on` or `blocks`, so a reader shown a dependency can be
    // shown what it is doing without a second call per id. Derived, never stored. An id matching no task
    // has no entry here — the same case that keeps `blocked` true, and a reader showing the handle with
    // no status beside it is telling the truth about it.
    #[serde(default)]
    pub link_statuses: Vec<TaskLinkResponse>,
    pub blocked: bool,
    // The checklist, in the order it was written. Empty for a task nobody broke down, which is most of
    // them. Carried in full — titles and texts — because the card that draws it is drawn from this
    // response and there is no second call from the browser for it.
    // Ids of the documents this task references — and ONLY the ids. Unlike everything else here, a document
    // is not held in memory on the server, so resolving these would mean a database read on every board
    // read: the count is drawn from this list without a request, and a name or a text is fetched when
    // somebody opens one.
    //
    // An id may name a document that has since been deleted. Nothing prunes the list, because deletion is
    // undoable — a reference silently dropped would not come back when the document was restored.
    #[serde(default)]
    pub documents: Vec<String>,
    #[serde(default)]
    pub subtasks: Vec<crate::subtasks::SubtaskResponse>,
    // The builds this work produced, oldest first — see `TaskGhActionResponse`. Empty for most tasks, which
    // is why the card draws nothing at all rather than an empty heading.
    //
    // Carried whole in the board snapshot, unlike a document reference: there is nothing to resolve, the
    // entry IS the link and its name, so the dialog can draw it without a request.
    #[serde(default)]
    pub gh_actions: Vec<TaskGhActionResponse>,
    #[serde(default)]
    pub comments: Vec<TaskCommentResponse>,
    pub created_unix_seconds: i64,
    pub updated_unix_seconds: i64,
    // When the task left Todo, and absent while it is there. Absent too on a task that left it before the
    // moment was recorded — tell the two apart by `status`.
    #[serde(default)]
    pub started_unix_seconds: Option<i64>,
    // When the task landed in Done, and absent whenever it is not there. Home shows it, and it is what
    // the seven-day archive window is measured from — a task closed longer ago than that is not returned
    // at all.
    pub closed_unix_seconds: Option<i64>,
    // When it was deleted, and absent for a task that is not.
    //
    // **Deleted work travels to the client and is hidden THERE.** That is deliberate: filtering it out on the
    // server would make it unfindable, and being findable after deletion is the entire reason the flag exists
    // rather than a row being removed. The board draws only what is not deleted; a search shows what is,
    // marked as gone.
    pub deleted_unix_seconds: Option<i64>,
}

// A whole board in one response, **most urgent first and oldest first within one priority**.
//
// Ordered by the server rather than by whoever draws it, so the board, the Goals screen and every agent
// reading `tasks_list` agree about what is at the top — and so a client that only groups the list into
// columns is already right.
//
// Not paged and not capped: a project's tasks are a hand-written list held entirely in memory, and
// Home renders all the columns at once anyway.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct TasksResponse {
    pub tasks: Vec<TaskResponse>,
}

// The exact task a handle names, plus which board it is on.
//
// Answered by the server rather than found in the loaded board, and that is the point: work closed more
// than seven days ago is not in the board read at all, and a handle can name a project other than the one
// on screen. Both are exactly when looking a number up is worth doing.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct FindTaskResponse {
    pub task: Option<TaskResponse>,
    // Set instead of `task` when the query named a GOAL — either written `RMS-G7`, or a bare number that
    // turned out to be a goal's. One counter serves both kinds per project, so a number is one or the
    // other and never both. Search is the only way to reach an archived goal, which is why it answers for
    // goals at all.
    pub goal: Option<crate::goals::GoalResponse>,
    // The PREFIX, like every other `project` on this wire. There is no internal id here, and there was
    // one beside this field until it was noticed that nothing had ever read it.
    pub project: String,
    pub project_name: String,
    // True when the hit is older than the project's archive window, so it is NOT on the board. Said out
    // loud, or somebody goes hunting through the columns for a card that is not drawn.
    pub archived: bool,
    // Why there is no task, in words. Empty on a hit — an id that is not there has to come back as a
    // message, never as an empty result that reads like "there is nothing there".
    pub not_found: String,
}

// Move one task to another column.
//
// The FIRST write the browser makes about the state of the board, and a deliberate change of mind rather
// than an oversight: dragging a card between columns is the one gesture a board is expected to have, and
// refusing it taught people the screen was broken rather than that it was a viewer. Everything else about a
// task still arrives through `/mcp` — its text, its type, who is on it, its thread.
//
// Two things make this safe to open up where the rest is not. The move is one field with a closed set of
// values, validated against the project like any other status change; and unlike MCP, this side HAS a
// session, so the comment a landing needs is signed by whoever dragged the card instead of by a `who` the
// caller made up.
#[derive(MyHttpInput)]
pub struct MoveTaskInputModel {
    #[http_body(name: "taskId", description: "Which task to move, by id — RMS-42")]
    pub task_id: String,
    #[http_body(name: "status", description: "The column to move it to, by column id")]
    pub status: String,
    #[http_body(
        name: "comment",
        description: "What was actually done. REQUIRED when the move lands the task in `done`, refused as a move without it — the same rule tasks_update obeys, for the same reason: the Done column is what makes a board worth reading months later"
    )]
    pub comment: Option<String>,
}

#[derive(MyHttpInput)]
pub struct FindTaskInputModel {
    #[http_body(name: "query", description: "A task id such as RMS-42 or RMS-000042, or a goal id such as RMS-G7")]
    pub query: String,
}

#[derive(MyHttpInput)]
pub struct GetTasksInputModel {
    #[http_body(name: "project", description: "Which project's board to read, by prefix — RMS")]
    pub project: String,
    #[http_body(
        name: "includeArchived",
        description: "Include work closed longer ago than the project's archive window. Omitted gives the live board, which is what a board screen wants; the Goals screen asks for everything, because a goal outlives the window and its list has to agree with its own counters"
    )]
    pub include_archived: Option<bool>,
}

/// The default archive window in days, when a project has not chosen one.
///
/// Duplicated from the server's `ARCHIVE_AFTER` on purpose: this is the wire side of the same rule, and the
/// two are pinned together by the tests either side rather than by a shared constant nobody would notice
/// changing.
pub const DEFAULT_ARCHIVE_DAYS: i64 = 7;

/// Whether a task has aged off the board, decided from what a client actually holds.
///
/// The client needs this now because a pushed snapshot carries a project's WHOLE history — a goal's list has
/// to agree with the counters beside it, and those count archived work. So the board screen does the
/// filtering the server used to do for it, and does it from the same rule: closed, and closed longer ago than
/// the project's window.
///
/// A task in `done` with no close moment counts as NOT archived, exactly as on the server: the moment is
/// written on the way in, and being lenient about a gap in the data means work cannot silently vanish.
pub fn is_task_archived(
    task: &TaskResponse,
    archive_days: Option<i32>,
    now_unix_seconds: i64,
) -> bool {
    let Some(closed) = task.closed_unix_seconds else {
        return false;
    };

    // A window that makes no sense reads as the default, the same leniency the server applies: writes
    // validate, and one bad row must not empty a board.
    let days = match archive_days {
        Some(days) if days > 0 => days as i64,
        _ => DEFAULT_ARCHIVE_DAYS,
    };

    now_unix_seconds - closed > days * 24 * 60 * 60
}

#[cfg(test)]
mod tests {
    use super::*;

    fn done_task(closed_unix_seconds: Option<i64>) -> TaskResponse {
        TaskResponse {
            id: "RMS-1".to_string(),
            project: "P".to_string(),
            text: "text".to_string(),
            status: crate::projects::COLUMN_ID_DONE.to_string(),
            priority: String::new(),
            kind: None,
            goal: None,
            goal_name: None,
            goal_color: None,
            assignee: None,
            assignee_name: None,
            labels: Vec::new(),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            link_statuses: Vec::new(),
            blocked: false,
            documents: Vec::new(),
            subtasks: Vec::new(),
            gh_actions: Vec::new(),
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            started_unix_seconds: None,
            closed_unix_seconds,
            deleted_unix_seconds: None,
        }
    }

    const NOW: i64 = 1_000 * 24 * 60 * 60;

    #[test]
    fn the_window_is_the_projects_and_the_default_is_a_week() {
        let six_days = done_task(Some(NOW - 6 * 24 * 60 * 60));
        let eight_days = done_task(Some(NOW - 8 * 24 * 60 * 60));

        assert!(!is_task_archived(&six_days, None, NOW));
        assert!(is_task_archived(&eight_days, None, NOW));

        // A project that says two days archives what the default would still be showing.
        assert!(is_task_archived(&six_days, Some(2), NOW));
        // And one that says thirty keeps what the default would have hidden.
        assert!(!is_task_archived(&eight_days, Some(30), NOW));
    }

    /// A nonsense window reads as the default rather than archiving everything the instant it closes.
    #[test]
    fn a_nonsense_window_falls_back() {
        let two_days = done_task(Some(NOW - 2 * 24 * 60 * 60));

        assert!(!is_task_archived(&two_days, Some(0), NOW));
        assert!(!is_task_archived(&two_days, Some(-5), NOW));
    }

    /// Done with no close moment must not vanish — the same leniency the server applies.
    #[test]
    fn done_without_a_close_moment_is_not_archived() {
        assert!(!is_task_archived(&done_task(None), Some(1), NOW));
    }
}
