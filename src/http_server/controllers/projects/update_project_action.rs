use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::UpdateProjectInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/update",
    controller: "Projects",
    summary: "Rename a project or move its prefix",
    description: "Admin only. Renaming the prefix keeps the old one in the project history, so an id written under it can still be traced with the tasks_resolve_id MCP tool once another project takes that prefix over.",
    input_data: "UpdateProjectInputModel",
    result: [
        {status_code: 200, description: "Updated"},
        {status_code: 400, description: "Bad name or prefix, or the prefix is taken"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct UpdateProjectAction {
    app: Arc<AppContext>,
}

impl UpdateProjectAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &UpdateProjectAction,
    input_data: UpdateProjectInputModel,
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

    crate::scripts::update_project(
        &action.app,
        &project_id,
        &input_data.name,
        &input_data.description,
        &input_data.prefix,
        input_data.archive_days,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
