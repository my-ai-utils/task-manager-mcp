use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::releases::{GetReleaseInputModel, ReleaseResponse};

use crate::app::AppContext;
use crate::mappers::release_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/releases/v1/get",
    controller: "Releases",
    summary: "Read one release",
    description: "One release of a project, by its id — RMS-R12 — or by its bare number. What a release's own page is drawn from: the address `release/{project}/{release}` hands its two halves straight to this, which is what makes a release something that can be linked to. A DELETED release is returned, with `deleted_unix_seconds` set, so that a link somebody kept says the release was deleted instead of reading like a typo. A release of another project is not found through this one's prefix.",
    input_data: "GetReleaseInputModel",
    result: [
        {status_code: 200, description: "The release", model: "ReleaseResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project, or no such release on it"},
    ]
)]
pub struct GetReleaseAction {
    app: Arc<AppContext>,
}

impl GetReleaseAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &GetReleaseAction,
    input_data: GetReleaseInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Prefix in, project out, access already established — see `require_project_by_prefix`. It is the
    // PROJECT access is decided on, which is why the address carries it on its own: a release of a board
    // the caller is not on is refused before anything is said about whether it exists.
    let project = crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let board = action.app.board.read();

    let release =
        match crate::scripts::find_release_of_project(&board, &project, &input_data.release) {
            Ok(release) => release,
            // The resolver's own words, passed through: they name a board the caller has just been shown
            // to be on, and say which half of the address is wrong.
            Err(err) => return Err(crate::http_server::errors::not_found(err)),
        };

    let response: ReleaseResponse = release_to_response(&release, &project, &board);

    HttpOutput::as_json(response).into_ok_result(true)
}
