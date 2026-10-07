use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::github::{GetGithubConnectionsInputModel, GithubConnectionsResponse};

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/connections",
    controller: "GitHub",
    summary: "The repositories connected to a project, and what their mirrors hold",
    description: "Two halves joined: what somebody configured, which is a row on the project and survives everything, and what has actually been pulled, which lives in this process's memory and survives nothing. That is why a connection can read as `needs-key` on a repository that was mirroring perfectly an hour ago — the configuration outlived the process, the key did not. THE KEY ITSELF IS NEVER RETURNED, by any call: `has_key` is the whole of what can be said about it. `state` is one of pending, pulling, ready, failed or needs-key, and a mirror that fails keeps whatever the last good pull left, so `files_amount` on a failed connection is what is still readable rather than a stale claim.",
    input_data: "GetGithubConnectionsInputModel",
    result: [
        {status_code: 200, description: "The connections", model: "GithubConnectionsResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct ListGithubConnectionsAction {
    app: Arc<AppContext>,
}

impl ListGithubConnectionsAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListGithubConnectionsAction,
    input_data: GetGithubConnectionsInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let project_id = {
        let board = action.app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, &input_data.project) {
            Ok(project) => project.id.clone(),
            Err(err) => return Err(not_found(err)),
        }
    };

    crate::auth::require_project_access(&action.app, ctx, &project_id).await?;

    let connections = crate::scripts::list_github_connections(&action.app, &input_data.project)
        .map_err(bad_request)?;

    HttpOutput::as_json(GithubConnectionsResponse { connections }).into_ok_result(true)
}
