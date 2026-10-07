use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::column_templates::DeleteColumnTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/column-templates/v1/delete",
    controller: "ColumnTemplates",
    summary: "Delete a column template",
    description: "Admin only. Refused while any project still follows it — a project whose template vanished would lose the middle of its board, and every task sitting in one of those columns would read as Todo. That is a large, silent consequence for a small click, so the projects have to be pointed elsewhere first, which makes it something chosen rather than discovered.",
    input_data: "DeleteColumnTemplateInputModel",
    result: [
        {status_code: 200, description: "Deleted"},
        {status_code: 400, description: "No such template, or projects still follow it"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct DeleteTemplateAction {
    app: Arc<AppContext>,
}

impl DeleteTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &DeleteTemplateAction,
    input_data: DeleteColumnTemplateInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::delete_column_template(&action.app, &input_data.id)
        .await
        .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
