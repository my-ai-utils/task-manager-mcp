use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::documents::{
    SkippedArchiveEntryResponse, UploadArchiveResponse, UploadDocumentResponse,
};
use task_manager_shared::github::SyncGithubInputModel;

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/sync",
    controller: "GitHub",
    summary: "Copy chosen files out of a connected repository into the project's documents",
    description: "The one crossing between a mirror and the project's own documents, and it is a COPY. What lands is a document with an id, a version, an author and a full history, exactly as an upload produces — it stops tracking the repository the moment it is written, and the next pull will not touch it. `paths` is relative to the connection's root: a folder takes everything under it, a file takes itself, and an empty list takes the whole mirror. `overrideExisting` is the difference between the two ways this gets used: off writes only what is not already there and reports the rest as skipped, which is safe to press repeatedly; on writes a new version over every chosen path, keeping ids and history. Off is the default because a button pressed by mistake should do the recoverable thing. PARTIAL BY DESIGN, like the archive upload: every file that can be written is written and the rest come back in `skipped` with a reason each. The author is the signed-in person, taken from the session.",
    input_data: "SyncGithubInputModel",
    result: [
        {status_code: 200, description: "What was written, and what was not", model: "UploadArchiveResponse"},
        {status_code: 400, description: "No such connection, nothing chosen, or a mirror with nothing in it"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct SyncGithubAction {
    app: Arc<AppContext>,
}

impl SyncGithubAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SyncGithubAction,
    input_data: SyncGithubInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project_id = {
        let board = action.app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, &input_data.project) {
            Ok(project) => project.id.clone(),
            Err(err) => return Err(not_found(err)),
        }
    };

    // Membership and the caller's identity in one call — the identity signs every version this writes,
    // and one sync can write hundreds.
    let user = crate::auth::require_project_access(&action.app, ctx, &project_id).await?;

    let (written, skipped) = crate::scripts::sync_github(
        &action.app,
        &input_data.project,
        &input_data.connection,
        &input_data.paths,
        input_data.folder.as_deref(),
        input_data.override_existing,
        &user.email,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::as_json(UploadArchiveResponse {
        documents: written
            .into_iter()
            .map(|row| UploadDocumentResponse {
                id: row.id,
                path: row.doc_path,
            })
            .collect(),
        skipped: skipped
            .into_iter()
            .map(|itm| SkippedArchiveEntryResponse {
                name: itm.name,
                reason: itm.reason,
            })
            .collect(),
    })
    .into_ok_result(true)
}
