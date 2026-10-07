use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::kind_templates::DeleteKindTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/kind-templates/v1/delete",
    controller: "KindTemplates",
    summary: "Delete a task-type template",
    description: "Admin only. Refused while any project still follows it. Losing a task type is not destructive the way losing a column is — a task pointing at one that is gone simply reads as having no type — but it still changes every following project at once, so the projects have to be pointed elsewhere first, which makes it something chosen rather than discovered.",
    input_data: "DeleteKindTemplateInputModel",
    result: [
        {status_code: 200, description: "Deleted"},
        {status_code: 400, description: "No such template, or projects still follow it"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct DeleteKindTemplateAction {
    app: Arc<AppContext>,
}

impl DeleteKindTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &DeleteKindTemplateAction,
    input_data: DeleteKindTemplateInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::delete_kind_template(&action.app, &input_data.id)
        .await
        .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
