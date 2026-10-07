use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::goals::SetGoalColorInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/goals/v1/color",
    controller: "Goals",
    summary: "Recolour a goal",
    description: "The ONLY write the browser makes about the board, and a deliberate exception rather than a crack in the rule that every change arrives through /mcp: a colour is presentation, not state. Same argument as a task type's colour, which is also picked with a mouse. Anyone who can see the project may recolour its goals — it changes how the board looks, not what it says. The colour must be one of the palette names; anything else is refused rather than stored and quietly read back as gray.",
    input_data: "SetGoalColorInputModel",
    result: [
        {status_code: 202, description: "Recoloured"},
        {status_code: 400, description: "Not a palette colour, or no such goal"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct SetGoalColorAction {
    app: Arc<AppContext>,
}

impl SetGoalColorAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetGoalColorAction,
    input_data: SetGoalColorInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project =
        crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    // The goal is named the way every other door names it — handle or bare number — so the handle is
    // composed here from the prefix the caller sent, before the script resolves it.
    let handle = {
        let board = action.app.board.read();

        let number = crate::scripts::resolve_goal_reference(&board, &project, &input_data.goal)
            .map_err(|err| bad_request(&err))?;

        crate::board::compose_goal_handle(&project.prefix, number)
    };

    crate::scripts::update_goal(
        &action.app,
        &handle,
        crate::scripts::GoalPatch {
            color: Some(input_data.color),
            ..Default::default()
        },
    )
    .await
    .map_err(|err| bad_request(&err))?;

    HttpOutput::Empty.into_ok_result(true)
}
