use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::kind_templates::KindTemplatesResponse;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/kind-templates/v1/list",
    controller: "KindTemplates",
    summary: "Every task-type template",
    description: "Admin only. A template is a named set of task types, defined once and followed by any number of projects — which is why types are configured here rather than on each project. `usedBy` counts the projects following each one: it says whether editing it is a small change or a large one, and a template with any followers cannot be deleted.",
    result: [
        {status_code: 200, description: "The templates", model: "KindTemplatesResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct ListKindTemplatesAction {
    app: Arc<AppContext>,
}

impl ListKindTemplatesAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListKindTemplatesAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    let board = action.app.board.read();

    let templates = board
        .get_kind_templates()
        .iter()
        .map(|template| {
            crate::mappers::kind_template_to_response(
                template,
                board.count_projects_using_kind_template(&template.id),
            )
        })
        .collect();

    HttpOutput::as_json(KindTemplatesResponse { templates }).into_ok_result(true)
}
