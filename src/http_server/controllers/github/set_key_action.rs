use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::github::SetGithubKeyInputModel;

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/key",
    controller: "GitHub",
    summary: "Give the server a key for a connected repository",
    description: "The call somebody makes after a restart, which is why it is open to any member of the project rather than to admins alone: without a key a private repository cannot be fetched or pushed, and making one person the only one who can supply it would make every deploy their problem. THE KEY IS HELD IN MEMORY ONLY — no table, no settings file, no log — so it is gone the next time this process starts, by design. What survives a restart is the CLONE on the volume: a connection that comes back `needs-key` still lists, reads and edits every file it has, and every local git command in it still runs — only reaching GitHub waits. The one case with nothing on disk at all is a private repository that has never been cloned, where the key is what makes the first clone possible. An empty string forgets the one being held. A pull starts immediately, because the point of typing a key is to watch the connection reach the remote again. Note what this means for a private repository: the key serves every member of the project for as long as it is held, and what it shares with them is whatever the token can do — `github_git` runs `git push` with this same key. A classic token needs the `repo` scope, which is read and write together, so it hands them both; a fine-grained one with Contents: Read clones and fetches and has its pushes refused, and Contents: Read and write is what a push takes.",
    input_data: "SetGithubKeyInputModel",
    result: [
        {status_code: 200, description: "Held"},
        {status_code: 400, description: "No such connection"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct SetGithubKeyAction {
    app: Arc<AppContext>,
}

impl SetGithubKeyAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &SetGithubKeyAction,
    input_data: SetGithubKeyInputModel,
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

    crate::scripts::set_github_key(
        &action.app,
        &input_data.project,
        &input_data.name,
        &input_data.key,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
