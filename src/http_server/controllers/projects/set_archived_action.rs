use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::projects::SetProjectArchivedInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/archived/set",
    controller: "Projects",
    summary: "Put a project away, or bring it back",
    description: "Admin only. An archived project drops out of every project picker and out of nothing else: it keeps its prefix, its tasks and documents still open by direct link, and a new project still cannot take that prefix over. Nothing is deleted, which is why this is a toggle and not a delete — deleting a project does not exist. Setting it twice is not an error, and the moment it was first put away is kept.",
    input_data: "SetProjectArchivedInputModel",
    result: [
        {status_code: 200, description: "Set"},
        {status_code: 400, description: "No such project"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SetArchivedAction {
    app: Arc<AppContext>,
}

impl SetArchivedAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetArchivedAction,
    input_data: SetProjectArchivedInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    // Prefix in — the only name a project has on the wire. Resolved here, once, so the script below keeps
    // speaking in ids. An archived project resolves exactly like a live one, which is what makes bringing
    // one back possible at all.
    let project_id = {
        let board = action.app.board.read();

        crate::scripts::resolve_project_by_prefix(&board, &input_data.project)
            .map_err(bad_request)?
            .id
            .clone()
    };

    crate::scripts::set_project_archived(&action.app, &project_id, input_data.archived)
        .await
        .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
