use task_manager_shared::documents::{DocumentIndexEntryResponse, DocumentResponse};

use crate::documents::DocumentIndexEntry;
use crate::postgres::DocumentDto;
use crate::scripts::{body_of, content_type_of};

/// Postgres -> wire, for one document a reader has opened.
///
/// **The text travels; the bytes do not.** `content` is `Some` only for a text document — a binary one is
/// fetched by the browser from the raw endpoint, where bytes go as bytes and an `<iframe>` can be pointed
/// straight at them. Putting base64 here as well would cost a third of the size of every file for a path
/// nothing uses.
///
/// There is no memory model in between, unlike every other type in this module: a document's PAYLOAD is never
/// cached, so the row is the model. Its index entry is cached — see [`document_entry_to_index_entry`].
///
/// `prefix` comes from outside because neither the row nor the index entry carries one — they hold the internal
/// project id, which does not cross this boundary. See `ProjectResponse` in the shared crate.
pub fn document_to_response(src: &DocumentDto, prefix: &str, brief: String) -> DocumentResponse {
    let body = body_of(src);

    DocumentResponse {
        id: src.id.clone(),
        project: prefix.to_string(),
        path: src.doc_path.clone(),
        content_type: content_type_of(src.content_type.as_deref(), &src.doc_path),
        is_binary: body.is_binary(),
        size: body.size_bytes(),
        content: body.as_text().map(|itm| itm.to_string()),
        version: src.version,
        created_unix_seconds: src.created.unix_microseconds / 1_000_000,
        updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
        updated_by: src.updated_by.clone(),
        brief,
    }
}

/// Memory -> wire, for one entry of the index.
///
/// From the in-memory index rather than from a row, which is the whole point: drawing a project's tree touches
/// no database at all, and a PDF's bytes stay where they are until somebody opens it.
pub fn document_entry_to_index_entry(
    src: &DocumentIndexEntry,
    prefix: &str,
    // Passed in rather than looked up, exactly as on the MCP side: this mapper knows nothing about the
    // app, and a signature that asks for the brief is what makes the compiler point at any listing that
    // forgot to join one.
    brief: String,
) -> DocumentIndexEntryResponse {
    DocumentIndexEntryResponse {
        id: src.id.clone(),
        project: prefix.to_string(),
        path: src.path.clone(),
        content_type: src.content_type.clone(),
        is_binary: src.is_binary,
        size: src.size,
        version: src.version,
        updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
        updated_by: src.updated_by.clone(),
        brief,
    }
}
