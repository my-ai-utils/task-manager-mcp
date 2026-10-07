use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::CreateProjectInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1",
    controller: "Projects",
    summary: "Create a project",
    description: "Admin only. The prefix becomes the first half of every task id on this board and must not be held by another project right now. A prefix some project used in the past and renamed away from is free to take — which is exactly why a task id is composed when it is read rather than stored.",
    input_data: "CreateProjectInputModel",
    result: [
        {status_code: 200, description: "Created"},
        {status_code: 400, description: "Bad name or prefix, or the prefix is taken"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct CreateProjectAction {
    app: Arc<AppContext>,
}

impl CreateProjectAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &CreateProjectAction,
    input_data: CreateProjectInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::create_project(
        &action.app,
        &input_data.name,
        &input_data.description,
        &input_data.prefix,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
