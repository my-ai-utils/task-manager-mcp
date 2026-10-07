use flurl::HttpVerb;
use task_manager_shared::documents::UploadArchiveResponse;
use task_manager_shared::github::{
    DeleteGithubConnectionInputModel, GetGithubConnectionsInputModel, GithubConnectionsResponse,
    PullGithubConnectionInputModel, PullGithubConnectionResponse, SetGithubConnectionInputModel,
    SetGithubKeyInputModel, SyncGithubInputModel,
};

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response};

/// The repositories connected to a project, and what their mirrors currently hold.
///
/// Read repeatedly rather than once: a pull is asynchronous, so this is also how a screen watches a
/// connection go from `pulling` to `ready`. **The key is never in the answer** — `has_key` is the whole
/// of what the server will say about it.
pub async fn get_github_connections(
    project: &str,
) -> Result<GithubConnectionsResponse, RequestError> {
    let request = GetGithubConnectionsInputModel {
        project: project.to_string(),
    };

    handle_http_response(authed("/api/github/v1/connections", HttpVerb::Post, request).await).await
}

/// Connect a repository, or change one that is connected. Admin only, server-side.
///
/// `key: None` leaves whatever key the server is holding, which is what makes editing a branch safe;
/// `Some("")` forgets it. The url is parsed on the server, so a `/tree/<branch>/<folder>` address can be
/// pasted whole and the branch and folder boxes left empty.
pub async fn set_github_connection(
    project: &str,
    name: &str,
    url: &str,
    branch: &str,
    path: &str,
    key: Option<String>,
) -> Result<(), RequestError> {
    let request = SetGithubConnectionInputModel {
        project: project.to_string(),
        name: name.to_string(),
        url: url.to_string(),
        branch: Some(branch.to_string()),
        path: Some(path.to_string()),
        key,
    };

    handle_http_empty(authed("/api/github/v1/connection/set", HttpVerb::Post, request).await).await
}

pub async fn delete_github_connection(project: &str, name: &str) -> Result<(), RequestError> {
    let request = DeleteGithubConnectionInputModel {
        project: project.to_string(),
        name: name.to_string(),
    };

    handle_http_empty(
        authed("/api/github/v1/connection/delete", HttpVerb::Post, request).await,
    )
    .await
}

/// Give the server a key for a connection — the call somebody makes after a restart.
///
/// Open to any member of the project rather than to admins alone, because a private repository's mirror
/// is empty until a key arrives and every deploy empties it again.
pub async fn set_github_key(project: &str, name: &str, key: &str) -> Result<(), RequestError> {
    let request = SetGithubKeyInputModel {
        project: project.to_string(),
        name: name.to_string(),
        key: key.to_string(),
    };

    handle_http_empty(authed("/api/github/v1/key", HttpVerb::Post, request).await).await
}

/// Refresh one connection now. Returns before the pull finishes.
///
/// What comes back is where to watch from: the connection's finished-listings count as it stood when the
/// ask was accepted. Read the connections back until that connection reports a higher `pull_no` and the
/// run is over — see [`PullGithubConnectionResponse`].
pub async fn pull_github_connection(
    project: &str,
    name: &str,
) -> Result<PullGithubConnectionResponse, RequestError> {
    let request = PullGithubConnectionInputModel {
        project: project.to_string(),
        name: name.to_string(),
    };

    handle_http_response(authed("/api/github/v1/pull", HttpVerb::Post, request).await).await
}

/// Copy chosen files out of a connected repository into the project's own documents.
///
/// `paths` is relative to the connection's root, and a folder path takes everything under it — the tree
/// sends what was ticked rather than expanding it here, so a folder ticked before a pull lands whatever
/// the mirror holds at the moment the button was pressed rather than whatever it held when it was drawn.
pub async fn sync_github(
    project: &str,
    connection: &str,
    paths: Vec<String>,
    folder: &str,
    override_existing: bool,
) -> Result<UploadArchiveResponse, RequestError> {
    let folder = folder.trim();

    let request = SyncGithubInputModel {
        project: project.to_string(),
        connection: connection.to_string(),
        paths,
        folder: if folder.is_empty() {
            None
        } else {
            Some(folder.to_string())
        },
        override_existing,
    };

    handle_http_response(authed("/api/github/v1/sync", HttpVerb::Post, request).await).await
}
