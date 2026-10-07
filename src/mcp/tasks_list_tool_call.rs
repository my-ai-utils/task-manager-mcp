use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::mcp::TaskView;

/// What a caller asked for when it passed `goal`, or did not.
#[derive(Clone, Copy)]
enum GoalFilter {
    Any,
    None,
    Number(i64),
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksListInput {
    #[property(description = "Which board to read, by project prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Return only tasks in this column, by column id. Omit for the whole board"
    )]
    pub status: Option<String>,
    #[property(description = "Return only tasks of this kind, by kind id. Omit for every kind")]
    pub kind: Option<String>,
    #[property(
        description = "Return only tasks assigned to this email, or to `AI`. Omit to ignore who is on them"
    )]
    pub assignee: Option<String>,
    #[property(description = "Return only tasks carrying this label. Omit to ignore labels")]
    pub label: Option<String>,
    #[property(
        description = "Return only the tasks of this goal, by goal id — `RMS-G7`, or a bare number. This is how you read one epic's work rather than a whole board. Pass an empty string for the opposite: only tasks that belong to no goal at all"
    )]
    pub goal: Option<String>,
    #[property(
        description = "Return only tasks that are ready to start — nothing in their `depends_on` is unfinished. Pass true when you are picking up work rather than surveying the board"
    )]
    pub only_unblocked: Option<bool>,
    #[property(
        description = "Include work closed more than seven days ago. Omitted or false gives you the live board, which is almost always what you want; pass true only when you are deliberately looking back — \"what did we ship last month\" — because a long-lived board has far more archived tasks than live ones"
    )]
    pub include_archived: Option<bool>,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct TasksListResponse {
    #[property(
        description = "The matching tasks, MOST URGENT FIRST and oldest first within one priority — the same order the board draws its columns in, so the top of this list is the top of the column. Not capped — a board is a hand-written list, not a data set"
    )]
    pub tasks: Vec<TaskView>,
    #[property(description = "Number of rows in `tasks`")]
    pub amount: i32,
}

pub struct TasksListHandler {
    app: Arc<AppContext>,
}

impl TasksListHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for TasksListHandler {
    const FUNC_NAME: &'static str = "tasks_list";
    const DESCRIPTION: &'static str = "Read a board. Every other task tool names a task by the `id` \
this returns, so call it before changing anything — and before creating anything, because a board is \
hand-written and a near-duplicate is a mistake rather than a second task. Each task also carries the \
derived `blocked` and `blocks`, which is how you tell what is ready to start from what is only \
waiting.\
\
By default this is the LIVE board: work closed more than seven days ago counts as archived and is left \
out, because Done is the only column that grows for ever. Nothing is deleted — an archived task is still \
reachable by its id, and `include_archived` brings the whole history back when you are deliberately \
looking backwards.";
}

#[async_trait::async_trait]
impl McpToolCall<TasksListInput, TasksListResponse> for TasksListHandler {
    async fn execute_tool_call(&self, model: TasksListInput) -> Result<TasksListResponse, String> {
        let board = self.app.board.read();
        let project = crate::scripts::resolve_project_by_prefix(&board, &model.project)?;

        // Filters are validated against the project before anything is matched, so a typo comes back
        // as "there is no such column" rather than as an empty board.
        let status = match &model.status {
            None => None,
            Some(status) => {
                let status = status.trim().to_lowercase();
                if !project.has_column(&status) {
                    return Err(format!(
                        "'{status}' is not a column of {} — call projects_list to see them",
                        project.prefix
                    ));
                }
                Some(status)
            }
        };

        let kind = match &model.kind {
            None => None,
            Some(kind) => {
                let kind = kind.trim().to_lowercase();
                if !project.has_kind(&kind) {
                    return Err(format!(
                        "'{kind}' is not a kind of {} — call projects_list to see them",
                        project.prefix
                    ));
                }
                Some(kind)
            }
        };

        let assignee = model
            .assignee
            .as_ref()
            .map(|itm| itm.trim().to_lowercase())
            .filter(|itm| !itm.is_empty());

        let label = model
            .label
            .as_ref()
            .map(|itm| itm.trim().to_lowercase())
            .filter(|itm| !itm.is_empty());

        // Three cases, and the third is the point: absent means "any goal", a handle means "this goal", and
        // an empty string means "no goal" — the tasks that stand on their own, which is what the Backlog
        // group on the screen is. Validated against the project first, so a mistyped goal comes back as a
        // message rather than as an empty board.
        let goal_filter = match model.goal.as_deref().map(str::trim) {
            None => GoalFilter::Any,
            Some("") => GoalFilter::None,
            Some(wanted) => {
                let resolved = crate::scripts::resolve_goal_reference(&board, &project, wanted)?;
                GoalFilter::Number(resolved)
            }
        };

        let only_unblocked = model.only_unblocked.unwrap_or(false);
        let include_archived = model.include_archived.unwrap_or(false);

        let tasks: Vec<TaskView> = board
            .tasks_of_project(&project.id)
            .into_iter()
            // Deleted work is in the collection so that SEARCHING can find it — see
            // `BoardInner::tasks_of_project` — and this is a listing, which must not. tasks_resolve_id is the
            // door for a deleted task, and it says so when it opens one.
            .filter(|task| !task.is_deleted())
            .collect::<Vec<_>>()
            .iter()
            // Compared against the *effective* status, so filtering by `todo` also finds the tasks
            // whose column was deleted — which is where the board shows them.
            .filter(|task| match &status {
                None => true,
                Some(wanted) => &project.effective_status(&task.status) == wanted,
            })
            .filter(|task| match &kind {
                None => true,
                Some(wanted) => {
                    project.effective_kind(task.kind.as_deref()).as_ref() == Some(wanted)
                }
            })
            .filter(|task| match &assignee {
                None => true,
                // Case-insensitive: an address is stored lower-cased and `AI` is stored as declared, so a
                // caller passing either spelling of either one has to match.
                Some(wanted) => task
                    .assignee
                    .as_ref()
                    .is_some_and(|itm| itm.eq_ignore_ascii_case(wanted)),
            })
            .filter(|task| match &label {
                None => true,
                Some(wanted) => task.labels.contains(wanted),
            })
            // Compared against the *effective* goal, like the status above: a number naming no goal reads
            // as standalone on the screen, so it has to read that way here too.
            .filter(|task| match goal_filter {
                GoalFilter::Any => true,
                GoalFilter::None => board.effective_goal(task).is_none(),
                GoalFilter::Number(wanted) => board
                    .effective_goal(task)
                    .is_some_and(|goal| goal.number == wanted),
            })
            .filter(|task| !only_unblocked || !board.is_blocked(task))
            // The same seven-day window Home uses, so the tool and the board agree on what "the board" is.
            .filter(|task| include_archived || !board.is_archived(task))
            .map(|task| TaskView::from_model(task, &project, &board))
            .collect();

        Ok(TasksListResponse {
            amount: tasks.len() as i32,
            tasks,
        })
    }
}
