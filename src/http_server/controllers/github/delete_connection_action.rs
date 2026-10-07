use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::github::DeleteGithubConnectionInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/connection/delete",
    controller: "GitHub",
    summary: "Detach a connected repository",
    description: "Admin only. Takes the connection off the project and forgets the key being held for it. THE CLONE ON DISK IS NOT DELETED: its folder stays under `git_repos_path`, and reclaiming it is a deliberate act on the host. A connection re-created under the same name adopts that folder — pressing Refresh on it is what deletes it and clones the repository the row now names. NOTHING THAT WAS SYNCED IS TOUCHED: a document copied out of the repository is a document of the project's own, with its own id and history, and disconnecting the repository it came from does not reach it. What disappears is the `github/<name>/` folder — the window, not what was carried through it.",
    input_data: "DeleteGithubConnectionInputModel",
    result: [
        {status_code: 200, description: "Detached"},
        {status_code: 400, description: "No such project, or no such connection"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct DeleteGithubConnectionAction {
    app: Arc<AppContext>,
}

impl DeleteGithubConnectionAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &DeleteGithubConnectionAction,
    input_data: DeleteGithubConnectionInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::delete_github_connection(&action.app, &input_data.project, &input_data.name)
        .await
        .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
