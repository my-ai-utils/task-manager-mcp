use std::sync::Arc;

use rust_extensions::base64::FromBase64;
use service_sdk::macros::use_my_http_server;
use task_manager_shared::documents::{
    SkippedArchiveEntryResponse, UploadArchiveInputModel, UploadArchiveResponse,
    UploadDocumentResponse,
};

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};
use crate::scripts::MAX_BINARY_LEN;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/documents/v1/upload-zip",
    controller: "Documents",
    summary: "Unpack a zip into a project's documents",
    description: "Upload a ZIP and get one document per file inside it. The sibling of /api/documents/v1/upload, and it exists because the one-file-at-a-time dialog is the wrong shape for what people actually have: a folder of documents. THE ARCHIVE IS NEVER STORED — it is a transport, and what lands is the files inside it, at paths built from the chosen folder plus each entry's own path, so the tree inside the zip becomes the tree in the project. An entry landing on a taken path writes a NEW VERSION of the document already there, keeping its id and its history, exactly as a single upload does. PARTIAL BY DESIGN: every entry that can be written is written and the rest come back in `skipped` with a reason each, because refusing a whole archive over one `.DS_Store` would make the feature useless. The author is the signed-in person, taken from the session.",
    input_data: "UploadArchiveInputModel",
    result: [
        {status_code: 200, description: "What was written, and what was not", model: "UploadArchiveResponse"},
        {status_code: 400, description: "Not valid base64, not a zip, or a folder that is not a path"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
    ]
)]
pub struct UploadArchiveAction {
    app: Arc<AppContext>,
}

impl UploadArchiveAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &UploadArchiveAction,
    input_data: UploadArchiveInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project_id = {
        let board = action.app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, &input_data.project) {
            Ok(project) => project.id.clone(),
            Err(err) => return Err(not_found(err)),
        }
    };

    // Membership, and the caller's identity in one call — the identity is what signs every version this
    // writes, and one archive can write hundreds.
    let user = crate::auth::require_project_access(&action.app, ctx, &project_id).await?;

    let archive = input_data
        .content_base64
        .from_base64()
        .map_err(|err| bad_request(format!("that archive did not decode: {err}")))?;

    // The archive travels the same base64 JSON road a single upload does, so it is bounded the same way. What
    // it UNPACKS to is a separate limit, counted while it is read — see `MAX_ARCHIVE_UNPACKED`, because the
    // sizes in a zip's headers are written by whoever made the zip.
    if archive.len() > MAX_BINARY_LEN {
        return Err(bad_request(format!(
            "that archive is {} bytes — the limit is {MAX_BINARY_LEN}",
            archive.len()
        )));
    }

    let (written, skipped) = crate::scripts::upload_archive(
        &action.app,
        &input_data.project,
        input_data.folder.as_deref(),
        &archive,
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
