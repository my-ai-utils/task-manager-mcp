use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

use crate::tasks::TaskCommentResponse;

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// A goal, as the Goals screen draws it.
//
// The container work is done around — an epic. `id` is its handle, `RMS-G7`: the number comes from the
// same per-project counter task numbers come from, so no number names both a task and a goal, and the `G`
// says which of the two you are holding.
//
// `status` is derived from whether the goal is closed, not stored, so it cannot disagree with
// `closed_unix_seconds`. Progress is `done_amount` out of `tasks_amount`, also derived on every read, and
// it counts ARCHIVED tasks too: a goal can only be closed once all its tasks are done, and by then the
// oldest of them have aged off the board — a count that skipped those would report finished work as
// half-done. Which is why the Goals screen must not recompute these numbers from the tasks it holds.
//
// The link lives on the TASK, not here: a task says which goal it is part of.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GoalResponse {
    pub id: String,
    // The board's PREFIX, like everywhere else on this wire — see `crate::projects::ProjectResponse`.
    pub project: String,
    pub name: String,
    pub description: String,
    // A palette name — `task_manager_shared::kind_color::KindColor` on the wire, read back with
    // `parse_or_default`. Purely visual: it is how a goal is recognised on the board without reading.
    pub color: String,
    // How urgent the goal is — the same five-value scale a task carries, read back with
    // `Priority::parse_or_default`. It is what decides where the goal sits on the Goals screen.
    #[serde(default)]
    pub priority: String,
    pub status: String,
    pub tasks_amount: i32,
    pub done_amount: i32,
    // The goal's own checklist, in the order it was written. Separate from `tasks_amount` /
    // `done_amount` and deliberately not folded into them: those count the goal's TASKS, which is what
    // decides whether it can close, and mixing a private breakdown into the number a goal is judged by
    // would make the counter mean two things.
    // Ids of the documents this goal references, and only the ids — see `TaskResponse::documents`.
    #[serde(default)]
    pub documents: Vec<String>,
    #[serde(default)]
    pub subtasks: Vec<crate::subtasks::SubtaskResponse>,
    // The releases this goal went out in, newest first — whole, not ids. A goal is the description of a
    // feature and a release is the record of it shipping, so this is the answer to "is it out, and in
    // which version of what". THIS link lives on the goal: it stores the numbers, and the releases
    // themselves are a project-level thing, read back here on every response. One that has since been
    // deleted is simply not in the list.
    #[serde(default)]
    pub releases: Vec<crate::releases::ReleaseResponse>,
    #[serde(default)]
    pub comments: Vec<TaskCommentResponse>,
    pub created_unix_seconds: i64,
    pub updated_unix_seconds: i64,
    // When work on the goal began, and absent until it has — the start of its bar on the timeline, where
    // `closed_unix_seconds` is the end. Set by whoever does the work, or stamped when the first task under
    // the goal leaves Todo. Absent on a goal from before the field existed too, which is why a reader that
    // must draw SOMETHING for a goal that is plainly under way falls back to `created_unix_seconds`.
    #[serde(default)]
    pub started_unix_seconds: Option<i64>,
    // When the goal was closed, and absent while it is open. What the archive window is measured from: a
    // goal closed longer ago than the project's window is not returned unless asked for.
    pub closed_unix_seconds: Option<i64>,
    // When it was deleted, and absent for a goal that is not — see `TaskResponse::deleted_unix_seconds`.
    pub deleted_unix_seconds: Option<i64>,
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GoalsResponse {
    pub goals: Vec<GoalResponse>,
}

#[derive(MyHttpInput)]
pub struct GetGoalsInputModel {
    #[http_body(name: "project", description: "Which project's goals to read, by prefix — RMS")]
    pub project: String,
    #[http_body(
        name: "includeArchived",
        description: "Include goals closed longer ago than the project's archive window. Omitted means the live list"
    )]
    pub include_archived: Option<bool>,
}

// Recolour one goal.
//
// The one write the browser makes about a goal, and it is deliberate rather than a crack in the rule that
// every change to the board arrives through MCP: a colour is presentation, not state — the same reason a
// task type's colour is picked in Settings with a mouse.
#[derive(MyHttpInput)]
pub struct SetGoalColorInputModel {
    #[http_body(name: "project", description: "Which project the goal is on, by prefix — RMS")]
    pub project: String,
    #[http_body(name: "goal", description: "Which goal, by id — RMS-G7 — or by its bare number")]
    pub goal: String,
    #[http_body(name: "color", description: "A palette colour name, e.g. blue")]
    pub color: String,
}
