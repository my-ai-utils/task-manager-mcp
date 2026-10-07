use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::column_templates::SaveColumnTemplateInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/column-templates/v1/save",
    controller: "ColumnTemplates",
    summary: "Save a column template whole",
    description: "Admin only. An empty id creates, an existing id replaces. The complete column list travels in one call — there is no add-column or remove-column — so the server never reconciles a delta, a half-finished edit is never visible to anyone, and cancelling costs nothing because nothing was sent. Validated as a whole: a duplicate id, a name that is empty, or `todo`/`done` (which every project already has) fails the call and leaves every project following the template untouched. On success every one of those projects picks the new columns up at once.",
    input_data: "SaveColumnTemplateInputModel",
    result: [
        {status_code: 200, description: "Saved"},
        {status_code: 400, description: "A duplicate or anchor column id, a missing name, or no such template"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SaveTemplateAction {
    app: Arc<AppContext>,
}

impl SaveTemplateAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SaveTemplateAction,
    input_data: SaveColumnTemplateInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::save_column_template(
        &action.app,
        &input_data.id,
        &input_data.name,
        &input_data.description,
        &input_data.columns,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
