use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::releases::{GetReleasesInputModel, ReleasesResponse};

use crate::app::AppContext;
use crate::mappers::release_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/releases/v1/list",
    controller: "Releases",
    summary: "Read a project's releases",
    description: "Every release of one project, newest first by the date of the release — what went out, and in which version of which microservice. Reads only: a release is recorded, corrected and deleted through /mcp, like everything else. Each one carries the goals that list it, with their names and colours, because the goal a release shipped has usually aged off the board by the time the release is read. Not cut at the archive window: a release does not age off, the list is the history. Deleted releases are left out.",
    input_data: "GetReleasesInputModel",
    result: [
        {status_code: 200, description: "The releases", model: "ReleasesResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct ListReleasesAction {
    app: Arc<AppContext>,
}

impl ListReleasesAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListReleasesAction,
    input_data: GetReleasesInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Prefix in, project out, access already established — see `require_project_by_prefix`.
    let project = crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let board = action.app.board.read();

    let releases = board
        .releases_of_project(&project.id)
        .iter()
        .map(|release| release_to_response(release, &project, &board))
        .collect();

    HttpOutput::as_json(ReleasesResponse { releases }).into_ok_result(true)
}
