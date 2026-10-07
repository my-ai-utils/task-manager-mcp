use flurl::HttpVerb;
use task_manager_shared::templates_transfer::{ImportTemplatesInputModel, ImportTemplatesResponse};

use crate::models::RequestError;

use super::{authed, handle_http_response};

/// Apply a templates YAML file to this instance.
///
/// The bytes go up as the raw body, exactly as the reader's file holds them — see the shared model. There
/// is no export counterpart here and there is not meant to be: a download is a navigation rather than a
/// `fetch`, so the browser follows `export_templates_url()` directly.
pub async fn import_templates(bytes: Vec<u8>) -> Result<ImportTemplatesResponse, RequestError> {
    let request = ImportTemplatesInputModel { content: bytes };

    handle_http_response(authed("/api/templates/v1/import", HttpVerb::Post, request).await).await
}
