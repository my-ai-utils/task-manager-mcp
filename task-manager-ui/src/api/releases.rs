use flurl::HttpVerb;
use task_manager_shared::releases::{GetReleasesInputModel, ReleasesResponse};

use crate::models::RequestError;

use super::{authed, handle_http_response};

/// Read a project's releases, newest first.
///
/// Reads only, like the board: a release is recorded, corrected and deleted through `/mcp`. Every one of
/// them comes back — there is no archive window to ask past, because a release does not age off.
pub async fn get_releases(project: &str) -> Result<ReleasesResponse, RequestError> {
    let request = GetReleasesInputModel {
        project: project.to_string(),
    };

    handle_http_response(authed("/api/releases/v1/list", HttpVerb::Post, request).await).await
}
