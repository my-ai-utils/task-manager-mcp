use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::tasks::{GetTasksInputModel, TasksResponse};

use crate::app::AppContext;
use crate::mappers::task_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/tasks/v1/list",
    controller: "Tasks",
    summary: "Read a board",
    description: "Reads a board. Every task mutation arrives through /mcp except one: moving a card between columns, which is /api/tasks/v1/move — see the note there for why that one is open. Tasks come back oldest first with `blocked` and `blocks` derived, their handles composed from the project's current prefix, and an unknown status already folded into Todo. Work closed longer ago than the project's archive window is left out unless `includeArchived` asks for it: a board wants the live window, and the Goals screen wants the lot, because a goal outlives that window and its list has to agree with its own counters.",
    input_data: "GetTasksInputModel",
    result: [
        {status_code: 200, description: "The board", model: "TasksResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct ListTasksAction {
    app: Arc<AppContext>,
}

impl ListTasksAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListTasksAction,
    input_data: GetTasksInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // The prefix is the only name a project has on this boundary, and this one call both resolves it and
    // establishes that the caller may see what it resolved to. Everything below speaks in ids.
    let project = crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let board = action.app.board.read();

    // Archived work is left out rather than paged: Done is the only column that grows for ever, and a
    // board nobody can read is a board nobody looks at. Nothing is deleted — the tasks are still there and
    // still reachable by id through MCP, and a caller that wants the history says so.
    let include_archived = input_data.include_archived.unwrap_or(false);

    let tasks = board
        .tasks_of_project(&project.id)
        .iter()
        .filter(|task| include_archived || !board.is_archived(task))
        .map(|task| task_to_response(task, &project, &board))
        .collect();

    HttpOutput::as_json(TasksResponse { tasks }).into_ok_result(true)
}
