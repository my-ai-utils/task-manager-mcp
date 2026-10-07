use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::tasks::MoveTaskInputModel;

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};
use crate::scripts::TaskPatch;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/tasks/v1/move",
    controller: "Tasks",
    summary: "Move a task to another column",
    description: "The one write the browser makes about the state of the board: a card dragged between columns. Everything else about a task still arrives through /mcp — its text, its type, who is on it, its thread — and this is deliberately the narrowest possible opening: one field, a closed set of values, validated against the project exactly as an MCP status change is. Landing a task in `done` REQUIRES `comment`, and unlike MCP that comment is signed by the session rather than by a name the caller passes: whoever dragged the card is who said it.",
    input_data: "MoveTaskInputModel",
    result: [
        {status_code: 202, description: "Moved"},
        {status_code: 400, description: "Not a column of this board, a task already there, or a landing with no comment"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such task"},
    ]
)]
pub struct MoveTaskAction {
    app: Arc<AppContext>,
}

impl MoveTaskAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &MoveTaskAction,
    input_data: MoveTaskInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let user = crate::auth::resolve_auth_user(&action.app, ctx).await?;

    // Resolved first, because the project to check access against is named by the handle rather than passed:
    // a caller must not be able to learn a task exists on a board they cannot see.
    let project_id = {
        let board = action.app.board.read();

        let resolved = crate::scripts::resolve_task(&board, &input_data.task_id)
            .map_err(|err| not_found(&err))?;

        resolved.project.id.clone()
    };

    // Not a 403 for a stranger: refusing differently from "no such task" would confirm the task exists,
    // which is the one thing somebody probing ids for a board they cannot see would learn.
    if !action
        .app
        .board
        .read()
        .get_project(&project_id)
        .map(|project| user.is_admin || project.is_member(&user.email))
        .unwrap_or(false)
    {
        return Err(not_found(format!(
            "no task {} you can see",
            input_data.task_id.trim()
        )));
    }

    // The same script the MCP tool calls, so every rule holds identically whichever door the change came
    // through — the column must exist on this board, and a landing owes an explanation.
    crate::scripts::update_task(
        &action.app,
        &input_data.task_id,
        TaskPatch {
            status: Some(input_data.status),
            comment: input_data.comment,
            // Signed by the session. This is the one thing this door does BETTER than MCP, which has no
            // identity to derive an author from and has to be handed one.
            comment_by: Some(user.email.clone()),
            ..Default::default()
        },
    )
    .await
    .map_err(|err| bad_request(&err))?;

    HttpOutput::Empty.into_ok_result(true)
}
