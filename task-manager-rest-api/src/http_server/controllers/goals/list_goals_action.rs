use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::goals::{GetGoalsInputModel, GoalsResponse};

use crate::app::AppContext;
use crate::mappers::goal_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/goals/v1/list",
    controller: "Goals",
    summary: "Read a project's goals",
    description: "The goals of one project, oldest first — the containers its work is organised under. Reads only: goals are created, renamed and closed through /mcp, like everything else. Progress comes back counted against the whole board and INCLUDES archived tasks, so a finished epic reads as finished; do not recompute it from a board read, which stops at the archive window. Goals closed longer ago than that window are left out unless `includeArchived` asks for them.",
    input_data: "GetGoalsInputModel",
    result: [
        {status_code: 200, description: "The goals", model: "GoalsResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct ListGoalsAction {
    app: Arc<AppContext>,
}

impl ListGoalsAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListGoalsAction,
    input_data: GetGoalsInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Prefix in, project out, access already established — see `require_project_by_prefix`.
    let project = crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let board = action.app.board.read();

    let include_archived = input_data.include_archived.unwrap_or(false);

    let goals = board
        .goals_of_project(&project.id)
        .iter()
        .filter(|goal| include_archived || !board.is_goal_archived(goal))
        .map(|goal| goal_to_response(goal, &project, &board))
        .collect();

    HttpOutput::as_json(GoalsResponse { goals }).into_ok_result(true)
}
