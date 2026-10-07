use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::board::{
    BoardInner, GoalModel, ProjectModel, ReleaseModel, TaskModel, compose_goal_handle,
    compose_release_handle, compose_task_handle, parse_goal_handle, parse_release_handle,
    parse_task_handle,
};
use crate::mcp::{GoalView, ReleaseView, TaskView};

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ResolveIdInput {
    #[property(
        description = "The task id to look up, as somebody wrote it, e.g. `RMS-42` or `RMS-000042`"
    )]
    pub id: String,
}

/// A project that once held the prefix, and what the same number means there now.
#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ArchivedMatch {
    #[property(
        description = "The prefix this project carries NOW. The id you asked about was written when it carried the one you searched for"
    )]
    pub project: String,
    #[property(description = "The project's name")]
    pub project_name: String,
    #[property(
        description = "What that task is called today, under this project's current prefix — this is the id to use from now on. Absent when the project never had a task with that number"
    )]
    pub id_today: Option<String>,
    #[property(description = "The task itself, when it exists")]
    pub task: Option<TaskView>,
    #[property(
        description = "The goal that number names on that project, when the number turned out to belong to a goal rather than a task"
    )]
    pub goal: Option<GoalView>,
    #[property(
        description = "The release that number names on that project, when the number turned out to belong to a release"
    )]
    pub release: Option<ReleaseView>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct ResolveIdResponse {
    #[property(description = "The id as it was asked about")]
    pub asked: String,
    #[property(
        description = "What this id means right now: the task on the board of whatever project currently holds the prefix. Absent when nobody holds it, or when that board has no such number"
    )]
    pub direct: Option<TaskView>,
    #[property(
        description = "Set instead of `direct` when the id names a GOAL rather than a task — either because it was written `RMS-G7`, or because the bare number turned out to be a goal's. Tasks and goals share one counter per project, so a number is one or the other and never both"
    )]
    pub direct_goal: Option<GoalView>,
    #[property(
        description = "Set instead of `direct` when the id names a RELEASE — written `RMS-R12`, or a bare number that turned out to be a release's. This is also how a deleted release is read back: it is in no listing, and it still answers here"
    )]
    pub direct_release: Option<ReleaseView>,
    #[property(
        description = "Projects that used to hold this prefix and have since renamed away from it. Non-empty means the id is AMBIGUOUS ACROSS TIME — `direct` is what it means today, and these are what it may have meant when it was written. Say so rather than silently answering about the wrong task"
    )]
    pub archived: Vec<ArchivedMatch>,
    #[property(description = "A one-line summary of the above, safe to repeat to a person as-is")]
    pub verdict: String,
}

pub struct ResolveIdHandler {
    app: Arc<AppContext>,
}

impl ResolveIdHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for ResolveIdHandler {
    const FUNC_NAME: &'static str = "tasks_resolve_id";
    const DESCRIPTION: &'static str = "Work out what a task id someone QUOTED actually refers to. \
Reach for it when an id comes from outside the current board — an older conversation, a commit \
message, a link a colleague pasted — rather than from a tool call you just made. A project's prefix \
can be renamed, and the freed prefix can then be taken by a different project, so the same string can \
mean one task today and another one last month. This answers both: what it means now, and which \
projects used to answer to it. If `archived` is not empty, tell the person before acting.\
\
Works for goals and releases too, written either way: `RMS-G7` names a goal outright and `RMS-R7` a \
release, and a bare `RMS-7` is answered with whichever of the three that number turned out to be — one \
counter serves them all, so it is never ambiguous. The answer comes back in `direct` for a task, \
`direct_goal` for a goal and `direct_release` for a release.";
}

#[async_trait::async_trait]
impl McpToolCall<ResolveIdInput, ResolveIdResponse> for ResolveIdHandler {
    async fn execute_tool_call(&self, model: ResolveIdInput) -> Result<ResolveIdResponse, String> {
        // Any spelling is accepted. `RMS-G7` says goal outright and `RMS-R7` says release; `RMS-42` says
        // only "number 42", and since one counter serves all three kinds, what that number names is a
        // lookup — which is exactly the question this tool exists to answer.
        let (parsed, asked_for) = if let Some(parsed) = parse_goal_handle(&model.id) {
            (parsed, AskedFor::Goal)
        } else if let Some(parsed) = parse_release_handle(&model.id) {
            (parsed, AskedFor::Release)
        } else {
            let parsed = parse_task_handle(&model.id).ok_or_else(|| {
                format!(
                    "'{}' is not an id — expected something like RMS-42, RMS-G7 for a goal or RMS-R7 for a release",
                    model.id
                )
            })?;

            (parsed, AskedFor::Number)
        };

        let board = self.app.board.read();

        let current_holder = board.get_project_by_prefix(&parsed.prefix);

        let direct = current_holder
            .as_ref()
            .map(|project| NamedByNumber::look_up(&board, project, parsed.number, asked_for));

        // Every project that ever held the prefix, minus the one holding it now — that one is `direct`
        // and listing it twice would read as two different answers.
        let archived: Vec<ArchivedMatch> = board
            .projects_ever_holding_prefix(&parsed.prefix)
            .iter()
            .filter(|project| project.prefix != parsed.prefix)
            .map(|project| {
                let found = NamedByNumber::look_up(&board, project, parsed.number, asked_for);

                ArchivedMatch {
                    project: project.prefix.clone(),
                    project_name: project.name.clone(),
                    id_today: found.id_today(project, parsed.number),
                    task: found
                        .task
                        .map(|task| TaskView::from_model(&task, project, &board)),
                    goal: found
                        .goal
                        .map(|goal| GoalView::from_model(&goal, project, &board)),
                    release: found
                        .release
                        .map(|release| ReleaseView::from_model(&release, project, &board)),
                }
            })
            .collect();

        let found_now = direct.as_ref().is_some_and(NamedByNumber::is_something);

        let verdict = build_verdict(&model.id, &parsed.prefix, found_now, &archived);

        let (direct, direct_goal, direct_release) = match (direct, current_holder) {
            (Some(found), Some(project)) => (
                found
                    .task
                    .map(|task| TaskView::from_model(&task, &project, &board)),
                found
                    .goal
                    .map(|goal| GoalView::from_model(&goal, &project, &board)),
                found
                    .release
                    .map(|release| ReleaseView::from_model(&release, &project, &board)),
            ),
            _ => (None, None, None),
        };

        Ok(ResolveIdResponse {
            asked: model.id,
            direct,
            direct_goal,
            direct_release,
            archived,
            verdict,
        })
    }
}

/// What the id was spelled as, which decides where it is allowed to land.
#[derive(Clone, Copy, PartialEq)]
enum AskedFor {
    /// `RMS-42` — a bare number, which may turn out to be any of the three kinds.
    Number,
    /// `RMS-G7` — a goal and nothing else.
    Goal,
    /// `RMS-R7` — a release and nothing else.
    Release,
}

/// What one number names on one project. At most one of the three is set: a number is handed out once.
struct NamedByNumber {
    task: Option<Arc<TaskModel>>,
    goal: Option<Arc<GoalModel>>,
    release: Option<Arc<ReleaseModel>>,
}

impl NamedByNumber {
    /// Look the number up as whatever the spelling allows, deleted or not.
    ///
    /// A marked id is looked up as its own kind only — `RMS-G7` must not answer with a task because a
    /// task happens to hold 7 somewhere the prefix used to live. A bare number is tried as each kind in
    /// turn, and the order does not matter: with one counter behind all three, at most one can answer.
    fn look_up(board: &BoardInner, project: &ProjectModel, number: i64, asked_for: AskedFor) -> Self {
        let task = match asked_for {
            AskedFor::Number => board.get_task_including_deleted(&project.id, number),
            AskedFor::Goal | AskedFor::Release => None,
        };

        let goal = match asked_for {
            AskedFor::Number | AskedFor::Goal if task.is_none() => {
                board.get_goal_including_deleted(&project.id, number)
            }
            _ => None,
        };

        let release = match asked_for {
            AskedFor::Number | AskedFor::Release if task.is_none() && goal.is_none() => {
                board.get_release_including_deleted(&project.id, number)
            }
            _ => None,
        };

        Self {
            task,
            goal,
            release,
        }
    }

    fn is_something(&self) -> bool {
        self.task.is_some() || self.goal.is_some() || self.release.is_some()
    }

    /// The id in the spelling of whichever kind was found, so what is handed back is usable as-is.
    fn id_today(&self, project: &ProjectModel, number: i64) -> Option<String> {
        if self.task.is_some() {
            return Some(compose_task_handle(&project.prefix, number));
        }

        if self.goal.is_some() {
            return Some(compose_goal_handle(&project.prefix, number));
        }

        if self.release.is_some() {
            return Some(compose_release_handle(&project.prefix, number));
        }

        None
    }
}

/// The one-line answer, so a caller does not have to reason about the combination of "found now" and
/// "found in history" to say something true.
fn build_verdict(asked: &str, prefix: &str, found_now: bool, archived: &[ArchivedMatch]) -> String {
    let live_archived: Vec<&ArchivedMatch> = archived
        .iter()
        .filter(|itm| itm.id_today.is_some())
        .collect();

    match (found_now, live_archived.is_empty()) {
        (true, true) => {
            format!("{asked} is that one, and it has never meant anything else.")
        }
        (true, false) => {
            let others: Vec<String> = live_archived
                .iter()
                .filter_map(|itm| itm.id_today.clone())
                .collect();
            format!(
                "{asked} points at that one today, but the prefix {prefix} used to belong to another project — the same id once meant {}. Check which one was intended.",
                others.join(" or ")
            )
        }
        (false, false) => {
            let others: Vec<String> = live_archived
                .iter()
                .filter_map(|itm| itm.id_today.clone())
                .collect();
            format!(
                "Nothing answers to {asked} now — no project holds the prefix {prefix}, or its board has no such number. It used to mean {}.",
                others.join(" or ")
            )
        }
        (false, true) => format!(
            "Nothing answers to {asked}, now or before. Either the id was mistyped or the task was deleted — note that goals are never deleted, so a goal id that resolves to nothing was never real."
        ),
    }
}
