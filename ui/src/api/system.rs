use flurl::{EmptyRequestModel, HttpVerb};
use task_manager_shared::system::DiagnosticsResponse;

use crate::models::RequestError;

use super::{authed, handle_http_response_opt};

/// What this deployment is configured with. `Ok(None)` when the caller is not an admin.
pub async fn get_diagnostics() -> Result<Option<DiagnosticsResponse>, RequestError> {
    let response = authed(
        "/api/system/v1/diagnostics",
        HttpVerb::Post,
        EmptyRequestModel,
    )
    .await;

    handle_http_response_opt(response).await
}
