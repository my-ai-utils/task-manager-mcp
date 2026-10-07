use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::SetProjectMembersInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/members/set",
    controller: "Projects",
    summary: "Set who may see this project",
    description: "Admin only. Replaces the whole set, which is how the screen works — a list of checkboxes saved at once — and means the caller never has to know the current state to change it. An email with no user row is accepted and simply grants nothing until that person is created.",
    input_data: "SetProjectMembersInputModel",
    result: [
        {status_code: 200, description: "Set"},
        {status_code: 400, description: "No such project"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SetMembersAction {
    app: Arc<AppContext>,
}

impl SetMembersAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetMembersAction,
    input_data: SetProjectMembersInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    // Prefix in — the only name a project has on the wire. Resolved here, once, so the script below keeps
    // speaking in ids; the resolver's message names the prefixes that exist, which is safe on an admin-only
    // endpoint and is why this one is passed through rather than dropped.
    let project_id = {
        let board = action.app.board.read();

        crate::scripts::resolve_project_by_prefix(&board, &input_data.project)
            .map_err(bad_request)?
            .id
            .clone()
    };

    crate::scripts::set_members(&action.app, &project_id, &input_data.members)
        .await
        .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
