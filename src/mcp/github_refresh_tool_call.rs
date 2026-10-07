use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GithubRefreshInput {
    #[property(description = "Which project the connection is on, by prefix, e.g. `RMS`")]
    pub project: String,
    #[property(
        description = "Which connected repository to re-clone — the connection's name, which is also the folder it appears as under `github/`. documents_list shows them"
    )]
    pub connection: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GithubRefreshResponse {
    #[property(
        description = "What the connection is now: `ready` when the clone is there and current, `needs-key` when GitHub refused what it was given, `failed` for anything else. On `needs-key` or `failed` the FOLDER IS UNCHANGED — the new clone is built beside the old one and only swapped in when it is complete — so everything that was readable still is"
    )]
    pub state: String,
    #[property(description = "Why it did not work, when it did not. Empty otherwise")]
    pub error: String,
    #[property(description = "The commit the working copy is on now, short form")]
    pub commit: String,
    #[property(description = "How many files the connection now lists")]
    pub files_amount: i32,
    #[property(
        description = "How many files are NOT listed — over the size limit, or at a path this product will not name. They are not missing, they are deliberately not shown"
    )]
    pub skipped_amount: i32,
    #[property(
        description = "HOW MANY OF THOSE FILES NOBODY HAS BRIEFED — and this is a to-do list rather than a statistic. A repository that has just arrived is a folder of texts nothing on this board knows anything about; work through them with documents_next_without_brief until it is zero, and every later question about what is in this repository is answered from a listing instead of by reading it again"
    )]
    pub without_brief: i32,
    #[property(
        description = "The connection's finished-pulls counter after this run. Only useful if your client gave up waiting: read it back from the connections list to see whether the run landed"
    )]
    pub pull_no: i64,
}

pub struct GithubRefreshHandler {
    app: Arc<AppContext>,
}

impl GithubRefreshHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GithubRefreshHandler {
    const FUNC_NAME: &'static str = "github_refresh";
    const DESCRIPTION: &'static str = "Clone a connected repository again and replace what is on this \
server with the result. THIS WAITS for the clone and answers with what came back — it is not a fetch \
asked for early.\
\
WHAT IT IS FOR. The ten-minute timer already fetches, so a connection is rarely stale by much. What a \
fetch cannot settle is a folder that is wrong in a way fetching does not touch: a force-pushed branch, \
a file that stopped being tracked, a working copy still on the branch it was cloned on because the \
connection's url or branch was edited afterwards. This settles all of them, because it takes the \
repository down again from scratch.\
\
NOTHING IS DELETED FIRST. The new clone is assembled beside the folder and swapped in when it is \
complete, so the connection stays listable and readable throughout, and a clone that fails — no key, no \
network, a repository that is not there — leaves everything exactly as it was with the reason on \
`state` and `error`. What the swap does discard is anything left in the old folder through github_git: \
an unpushed commit, a stash. Push those first.\
\
IT CAN TAKE MINUTES on a large repository, and git's own ceiling is five. If your client gives up \
waiting, the re-clone still finishes — it is not cancelled by the call going away.\
\
THEN READ WHAT ARRIVED. `without_brief` says how many of the files nobody has summarised; \
documents_next_without_brief hands them over one at a time and documents_set_brief files what you \
learned. That loop is what makes the next question about this repository answerable from documents_list \
rather than from reading it again.";
}

#[async_trait::async_trait]
impl McpToolCall<GithubRefreshInput, GithubRefreshResponse> for GithubRefreshHandler {
    async fn execute_tool_call(
        &self,
        model: GithubRefreshInput,
    ) -> Result<GithubRefreshResponse, String> {
        let outcome =
            crate::scripts::refresh_connection_now(&self.app, &model.project, &model.connection)
                .await?;

        Ok(GithubRefreshResponse {
            state: outcome.state.to_string(),
            error: outcome.error,
            commit: outcome.commit,
            files_amount: outcome.files_amount as i32,
            skipped_amount: outcome.skipped_amount as i32,
            without_brief: outcome.without_brief as i32,
            pull_no: outcome.pull_no,
        })
    }
}
