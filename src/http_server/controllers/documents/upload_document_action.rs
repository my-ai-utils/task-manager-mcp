use std::sync::Arc;

use rust_extensions::base64::FromBase64;
use service_sdk::macros::use_my_http_server;
use task_manager_shared::documents::{UploadDocumentInputModel, UploadDocumentResponse};

use crate::http_server::errors::{bad_request, not_found};
use crate::app::AppContext;
use crate::scripts::{DocumentBody, NewDocumentContent};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/documents/v1/upload",
    controller: "Documents",
    summary: "Upload a document",
    description: "Put a file into a project's documents. THE ONE WRITE THE BROWSER MAKES ABOUT A DOCUMENT, and the second exception in the product to 'MCP writes, the UI reads' — the first being a goal's colour. It is here because the alternative is not a person using MCP, it is a person unable to upload at all: a PDF on a laptop cannot reach an agent without being base64-ed by hand into a tool call. Everything else about a document — moving, deleting, restoring — is still MCP only. Uploading to a path that is taken writes a NEW VERSION of the document already there, keeping its id and its history, exactly as the MCP tool does; there is one write path and this is a second door into it. The author is the signed-in person, taken from the session rather than passed, which is the one thing this side can do better than MCP.",
    input_data: "UploadDocumentInputModel",
    result: [
        {status_code: 200, description: "The document as it now stands", model: "UploadDocumentResponse"},
        {status_code: 400, description: "Not a valid path, or not valid base64"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
    ]
)]
pub struct UploadDocumentAction {
    app: Arc<AppContext>,
}

impl UploadDocumentAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &UploadDocumentAction,
    input_data: UploadDocumentInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project_id = {
        let board = action.app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, &input_data.project) {
            Ok(project) => project.id.clone(),
            Err(err) => return Err(not_found(err)),
        }
    };

    // Membership, and the caller's identity in one call — the identity is what signs the version.
    let user = crate::auth::require_project_access(&action.app, ctx, &project_id).await?;

    let bytes = input_data
        .content_base64
        .from_base64()
        .map_err(|err| bad_request(format!("that file did not decode: {err}")))?;

    // Bytes always, never text — even for a Markdown file. The browser hands over a file, and what a file
    // holds is bytes; deciding it is "really text" here would mean guessing an encoding, and guessing wrong
    // stores mojibake that no later read can undo.
    //
    // The content type still comes from the browser or the path, so a `.md` uploaded this way is served and
    // rendered as Markdown — it is only the STORAGE that is honest about what arrived.
    let content = NewDocumentContent {
        body: DocumentBody::Binary(bytes),
        content_type: input_data.content_type,
    };

    let row = crate::scripts::upload_document(
        &action.app,
        &input_data.project,
        &input_data.path,
        content,
        &user.email,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::as_json(UploadDocumentResponse {
        id: row.id,
        path: row.doc_path,
    })
    .into_ok_result(true)
}
