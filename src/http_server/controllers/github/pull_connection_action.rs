use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::github::{PullGithubConnectionInputModel, PullGithubConnectionResponse};

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/github/v1/pull",
    controller: "GitHub",
    summary: "Refresh one connection now",
    description: "THIS CLONES THE REPOSITORY AGAIN AND REPLACES THE CONNECTION'S FOLDER WITH THE RESULT. It is not the ten-minute timer's fetch asked for early: that keeps the clone current, and this replaces it, which is what settles a folder a fetch cannot fix — a force-pushed branch, a file that stopped being tracked, a working copy left on the branch it was cloned on. THE FOLDER IS NOT DELETED FIRST: the new clone is assembled beside it and swapped in only when it is complete, so the connection stays listable and readable throughout, and a clone that FAILS — no key, no network — leaves everything exactly as it was, with the reason on the connection. What the swap does discard is anything somebody left in the old folder through `github_git`: an unpushed commit, a stash. IT RETURNS BEFORE THE PULL FINISHES — a repository can take half a minute to arrive, and a request held open for that is a request that times out somewhere in between. What comes back is a receipt: `pull_no`, the connection's finished-listings count as it was when the ask was accepted. Read the connections list until that connection reports a HIGHER one and the run is over, whatever it did — the state and the error on it then describe THIS run rather than the one before. A PULL THAT IS ALREADY RUNNING IS WAITED FOR, NOT SKIPPED: the ten-minute timer's fetch is a different, weaker operation, so a refresh arriving on top of one queues behind it and then re-clones — and the receipt is read after the claim is taken, so it cannot be satisfied by the run that was already going. If that run is still going twenty seconds later this answers 400 saying so, rather than holding the request open behind what may be a first clone of a large repository.",
    input_data: "PullGithubConnectionInputModel",
    result: [
        {status_code: 200, description: "A pull was asked for", model: "PullGithubConnectionResponse"},
        {status_code: 400, description: "No such connection"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "No access to this project"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct PullGithubConnectionAction {
    app: Arc<AppContext>,
}

impl PullGithubConnectionAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &PullGithubConnectionAction,
    input_data: PullGithubConnectionInputModel,
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

    let pull_no =
        crate::scripts::pull_github_connection(&action.app, &input_data.project, &input_data.name)
            .await
            .map_err(bad_request)?;

    HttpOutput::as_json(PullGithubConnectionResponse { pull_no }).into_ok_result(true)
}
