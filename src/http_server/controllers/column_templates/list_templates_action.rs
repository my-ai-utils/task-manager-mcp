use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::column_templates::ColumnTemplatesResponse;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/column-templates/v1/list",
    controller: "ColumnTemplates",
    summary: "Every column template",
    description: "Admin only. A template is a named set of columns, defined once and followed by any number of projects — which is why columns are configured here rather than on each project. `usedBy` counts the projects following each one: it says whether editing it is a small change or a large one, and a template with any followers cannot be deleted.",
    result: [
        {status_code: 200, description: "The templates", model: "ColumnTemplatesResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct ListTemplatesAction {
    app: Arc<AppContext>,
}

impl ListTemplatesAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListTemplatesAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    let board = action.app.board.read();

    let templates = board
        .get_column_templates()
        .iter()
        .map(|template| {
            crate::mappers::column_template_to_response(
                template,
                board.count_projects_using_template(&template.id),
            )
        })
        .collect();

    HttpOutput::as_json(ColumnTemplatesResponse { templates }).into_ok_result(true)
}
