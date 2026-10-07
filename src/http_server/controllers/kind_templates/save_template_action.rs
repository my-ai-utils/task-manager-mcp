use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::kind_templates::SaveKindTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/kind-templates/v1/save",
    controller: "KindTemplates",
    summary: "Save a task-type template whole",
    description: "Admin only. An empty id creates, an existing id replaces. The complete list travels in one call — there is no add-type or remove-type — so the server never reconciles a delta, a half-finished edit is never visible to anyone, and cancelling costs nothing because nothing was sent. Validated as a whole: a duplicate id, an empty name or an unknown colour fails the call and leaves every project following the template untouched. On success every one of those projects picks the new types up at once.",
    input_data: "SaveKindTemplateInputModel",
    result: [
        {status_code: 200, description: "Saved"},
        {status_code: 400, description: "A duplicate id, a missing name, an unknown colour, or no such template"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SaveKindTemplateAction {
    app: Arc<AppContext>,
}

impl SaveKindTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SaveKindTemplateAction,
    input_data: SaveKindTemplateInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::save_kind_template(
        &action.app,
        &input_data.id,
        &input_data.name,
        &input_data.description,
        &input_data.kinds,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
