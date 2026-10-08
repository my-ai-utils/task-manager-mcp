use flurl::HttpVerb;
use task_manager_shared::releases::{
    GetReleaseInputModel, GetReleasesInputModel, ReleaseResponse, ReleasesResponse,
};

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

/// Read one release, named the way its page's address names it: `release/{project}/{release}`.
///
/// Always the server, though the list above may already be in hand: a link is opened cold, by somebody
/// who was on no screen at all a moment ago. `release` is the id — `RMS-R12` — or its bare number, and a
/// deleted release comes back too, marked as deleted, so that a link somebody kept can say so.
pub async fn get_release(project: &str, release: &str) -> Result<ReleaseResponse, RequestError> {
    let request = GetReleaseInputModel {
        project: project.to_string(),
        release: release.to_string(),
    };

    handle_http_response(authed("/api/releases/v1/get", HttpVerb::Post, request).await).await
}
