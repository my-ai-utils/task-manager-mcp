use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::SetProjectKindTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/kind-template/set",
    controller: "Projects",
    summary: "Choose which task-type template this project follows",
    description: "Admin only. The whole of a project's task-type configuration: types themselves are configured once per template, under Settings, and a project only points at one. An empty id means the project follows none and therefore has no task types — legitimate, since a type is optional on a task. A task pointing at a type the new template does not have keeps its stored value and reads as having none, so putting that type back brings it home.",
    input_data: "SetProjectKindTemplateInputModel",
    result: [
        {status_code: 200, description: "Set"},
        {status_code: 400, description: "No such project, or no such template"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SetKindTemplateAction {
    app: Arc<AppContext>,
}

impl SetKindTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetKindTemplateAction,
    input_data: SetProjectKindTemplateInputModel,
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

    crate::scripts::set_kind_template(
        &action.app,
        &project_id,
        &input_data.kind_template_id,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
