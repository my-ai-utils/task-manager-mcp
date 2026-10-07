use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::templates_transfer::{
    ImportTemplatesInputModel, ImportTemplatesResponse, SkippedTemplateResponse,
};

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/templates/v1/import",
    controller: "Templates",
    summary: "Apply a templates YAML file to this instance",
    description: "The other half of /api/templates/v1/export: the YAML goes up as the raw body and every template in it is applied. KEYED BY ID, AND A TEMPLATE ALREADY HERE IS REPLACED — that is the point of the feature, and also its sharp edge, because a template is followed by projects and replacing one changes every board that follows it in the same write. Nothing is deleted by a replacement: a task parked in a column the new version does not have keeps its stored status and reads as Todo, and putting the column back brings it home. How many projects each replacement touched comes back in `notes`. A template id is never re-minted — it is what a project's column_template_id points at, so an id in the file is the identity, and a template with no id is refused rather than generated. Validation is the same code the Settings screen goes through, so a file cannot install something the UI would refuse to save. PARTIAL BY DESIGN: a template that fails validation comes back in `skipped` with a reason and the rest still arrives. Admin only.",
    input_data: "ImportTemplatesInputModel",
    result: [
        {status_code: 200, description: "What was applied, what was not, and what it changed", model: "ImportTemplatesResponse"},
        {status_code: 400, description: "Not a templates export, or more than one import may carry"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct ImportTemplatesAction {
    app: Arc<AppContext>,
}

impl ImportTemplatesAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ImportTemplatesAction,
    input_data: ImportTemplatesInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    let outcome = crate::scripts::import_templates(&action.app, &input_data.content)
        .await
        .map_err(bad_request)?;

    HttpOutput::as_json(ImportTemplatesResponse {
        column_templates_created: outcome.column_templates_created as i32,
        column_templates_replaced: outcome.column_templates_replaced as i32,
        kind_templates_created: outcome.kind_templates_created as i32,
        kind_templates_replaced: outcome.kind_templates_replaced as i32,
        skipped: outcome
            .skipped
            .into_iter()
            .map(|itm| SkippedTemplateResponse {
                name: itm.name,
                reason: itm.reason,
            })
            .collect(),
        notes: outcome.notes,
    })
    .into_ok_result(true)
}
