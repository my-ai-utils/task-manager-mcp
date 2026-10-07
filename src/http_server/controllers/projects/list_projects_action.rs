use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::ProjectsResponse;

use crate::app::AppContext;
use crate::mappers::project_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/list",
    controller: "Projects",
    summary: "The projects I may see",
    description: "Feeds the project dropdown on Home and the list in Projects setup. Membership decides what comes back; an admin sees every project without being a member of any. Someone who is a member of nothing gets an empty list rather than a 403 — the UI shows them a note to ask an admin. Archived projects come back too, carrying `archived: true`: this one list feeds both a dropdown that must hide them and a setup table that must show them under a toggle, so the flag travels and each reader decides.",
    result: [
        {status_code: 200, description: "The visible projects", model: "ProjectsResponse"},
        {status_code: 401, description: "Not authenticated"},
    ]
)]
pub struct ListProjectsAction {
    app: Arc<AppContext>,
}

impl ListProjectsAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListProjectsAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let user = crate::auth::resolve_auth_user(&action.app, ctx).await?;

    let board = action.app.board.read();

    let projects = board
        .projects_visible_to(&user.email, user.is_admin)
        .iter()
        .map(|project| {
            // The template is looked up per project rather than passed as a map: the list is short, the
            // lookup is a hash hit, and threading a collection through the mapper would make every
            // caller know about templates.
            let column_template = project
                .column_template_id
                .as_ref()
                .and_then(|id| board.get_column_template(id));

            let kind_template = project
                .kind_template_id
                .as_ref()
                .and_then(|id| board.get_kind_template(id));

            project_to_response(
                project,
                board.tasks_amount(&project.id),
                column_template.as_deref(),
                kind_template.as_deref(),
            )
        })
        .collect();

    HttpOutput::as_json(ProjectsResponse { projects }).into_ok_result(true)
}
