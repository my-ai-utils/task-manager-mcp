use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::documents::{FindDocumentResponse, GetDocumentInputModel};

use crate::app::AppContext;
use crate::http_server::errors::forbidden;
use crate::mappers::document_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/documents/v1/get",
    controller: "Documents",
    summary: "Read one document",
    description: "One document with its TEXT, and metadata for one that has none. This is the fetch-on-demand half of the design: the index carries no payloads, and a text arrives here when somebody opens it. A BINARY document comes back with `content` absent and `isBinary` true — its bytes are not in this response on purpose, because the browser fetches those from /api/documents/v1/raw, where an <iframe> or an <img> can be pointed straight at them and nothing pays for base64. A miss is prose rather than an empty body: a reference to a deleted document reads as 'it is in the trash', which is a different thing to be told than 'it is not there', because restoring it is one MCP call away.",
    input_data: "GetDocumentInputModel",
    result: [
        {status_code: 200, description: "The document, or why there is none", model: "FindDocumentResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
    ]
)]
pub struct GetDocumentAction {
    app: Arc<AppContext>,
}

impl GetDocumentAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &GetDocumentAction,
    input_data: GetDocumentInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project =
        crate::auth::require_project_by_prefix(&action.app, ctx, &input_data.project).await?;

    let project_id = project.id.as_str();

    let telemetry = service_sdk::my_telemetry::MyTelemetryContext::create_empty();

    // A reference off a task or a goal is a url — `raw/TM/document/<id>` or `raw/TM/github/<path>` — and it
    // is what the browser has in hand when somebody clicks one. Unwrapped here rather than at the caller,
    // so that following a reference and opening a row from the tree are one request and not two shapes of
    // one.
    let unwrapped = crate::scripts::unwrap_document_reference(&input_data.id);
    let id = unwrapped.as_str();

    // A file in a connected repository is named by an id of its own shape and has NO ROW IN POSTGRES —
    // see `task_manager_shared::github::MIRROR_ID_PREFIX`. Intercepted before the repository, which would otherwise
    // miss and report "No document github:PROP:…", a sentence that reads as a bug rather than as an
    // answer. This is the same interception `resolve_document` makes for MCP; the two doors must agree.
    if let Some((id_prefix, mirror_path)) = crate::scripts::parse_mirror_document_id(id) {
        // The id carries the project it was minted for, so the same disagreement the row check below
        // guards against is guarded here: naming one project must not read another's.
        if !id_prefix.eq_ignore_ascii_case(&project.prefix) {
            return Err(forbidden("That document belongs to another project"));
        }

        let found =
            crate::scripts::read_mirror_document(&action.app, &project.prefix, mirror_path).await;

        return match found {
            Ok(row) => HttpOutput::as_json(FindDocumentResponse {
                document: Some(document_to_response(
                    &row,
                    &project.prefix,
                    action.app.briefs.text_of(row.content_hash.as_deref()),
                )),
                in_trash: false,
                not_found: String::new(),
            })
            .into_ok_result(true),
            // A miss here is prose too, and it is DIFFERENT prose: the file may have gone upstream, or
            // the connection may be waiting for a key. There is no trash to ask about — nothing here
            // was ever this project's to delete.
            Err(err) => HttpOutput::as_json(FindDocumentResponse {
                document: None,
                in_trash: false,
                not_found: err,
            })
            .into_ok_result(true),
        };
    }

    if let Some(row) = action.app.documents_repo.get_by_id(id, &telemetry).await {
        // The caller passed a project AND an id, and the id is what actually finds the row — so the two have
        // to be checked to agree. Without this, membership of one project would read any document of any
        // other: the access check above is about the project that was NAMED, not the one the document is on.
        if row.project_id != project_id {
            return Err(forbidden("That document belongs to another project"));
        }

        return HttpOutput::as_json(FindDocumentResponse {
            document: Some(document_to_response(
                &row,
                &project.prefix,
                action.app.briefs.text_of(row.content_hash.as_deref()),
            )),
            in_trash: false,
            not_found: String::new(),
        })
        .into_ok_result(true);
    }

    // Not live. The trash is asked ONLY to say which of the two misses this is — the row itself is never
    // returned, because the browser does not show trashed documents at all.
    let in_trash = action
        .app
        .documents_repo
        .get_trashed(id, &telemetry)
        .await
        .map(|itm| itm.project_id == project_id)
        .unwrap_or(false);

    let not_found = if in_trash {
        "This document has been deleted — it is in the trash. Restoring it is an MCP call.".to_string()
    } else {
        format!("No document {id}")
    };

    HttpOutput::as_json(FindDocumentResponse {
        document: None,
        in_trash,
        not_found,
    })
    .into_ok_result(true)
}
