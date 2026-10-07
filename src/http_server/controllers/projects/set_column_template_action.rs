use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::SetProjectColumnTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/column-template/set",
    controller: "Projects",
    summary: "Choose which column template this project follows",
    description: "Admin only. The whole of a project's column configuration: columns themselves are configured once per template, under Settings, and a project only points at one. An empty id means the project follows none, and its board is then just Todo and Done — a legitimate state, not an error. Every task parked in a column the new template does not have keeps its stored status and reads as Todo until a column with that id exists again.",
    input_data: "SetProjectColumnTemplateInputModel",
    result: [
        {status_code: 200, description: "Set"},
        {status_code: 400, description: "No such project, or no such template"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SetColumnTemplateAction {
    app: Arc<AppContext>,
}

impl SetColumnTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetColumnTemplateAction,
    input_data: SetProjectColumnTemplateInputModel,
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

    crate::scripts::set_column_template(
        &action.app,
        &project_id,
        &input_data.column_template_id,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
