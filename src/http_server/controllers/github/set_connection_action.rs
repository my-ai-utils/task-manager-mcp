use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::github::SetGithubConnectionInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/connection/set",
    controller: "GitHub",
    summary: "Connect a GitHub repository to a project, or change one",
    description: "Admin only. A name that is taken is an edit, a name that is free is a new connection — the same shape every other setup call here has. The repository appears in the project's documents as the folder `github/<name>/`, and what is behind those paths is a real `git clone` on the volume: it is served by the same list and read calls the project's own documents are served by, and by nothing that writes: a connected repository is read-only here, because a refresh deletes the folder and clones it again. Copying a file into the project's own documents, with an id and a history of its own, is a sync. THE KEY IS NOT STORED. It is held in this process's memory and written to no table, so a database dump carries no credential — a private connection comes up `needs-key` after a restart and cannot fetch until somebody supplies it again, but the clone and every file in it survive on the volume, so it still lists and reads. Omitting `key` leaves whatever key is already held, so editing a branch does not sign the connection out; an empty string forgets it. The url is parsed rather than demanded in pieces: a /tree/<branch>/<folder> address fills the branch and folder boxes by itself, and anything typed beside it wins. A pull starts immediately and is NOT waited for — read the state back from the connections list.",
    input_data: "SetGithubConnectionInputModel",
    result: [
        {status_code: 200, description: "Connected"},
        {status_code: 400, description: "No such project, an unusable name, or a url that is not a github.com repository"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct SetGithubConnectionAction {
    app: Arc<AppContext>,
}

impl SetGithubConnectionAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetGithubConnectionAction,
    input_data: SetGithubConnectionInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::set_github_connection(
        &action.app,
        &input_data.project,
        &input_data.name,
        &input_data.url,
        input_data.branch.as_deref(),
        input_data.path.as_deref(),
        input_data.key.as_deref(),
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
