use std::sync::Arc;

use mcp_server_middleware::*;
use serde::{Deserialize, Serialize};

use crate::app::AppContext;
use crate::github::{git, workdir};

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GithubGitInput {
    #[property(description = "Which board the repository is connected to, by project prefix")]
    pub project: String,
    #[property(
        description = "Which connected repository, by the name it appears under — the `<connection>` in a `github/<connection>/…` path. documents_list shows them"
    )]
    pub connection: String,
    #[property(
        description = "The git command, written the way you would type it: `git status --short`, `git commit -m \"fixed the thing\"`, `git push`. It must start with `git` and nothing else is accepted. It is NOT run through a shell, so `;`, `&&`, `|` and `$(…)` are ordinary characters in an argument rather than a way to run something else — and every real git subcommand, including `-c` and aliases, works. Quotes group an argument the usual way, which is how a commit message with spaces in it survives"
    )]
    pub command: String,
}

#[derive(ApplyJsonSchema, Debug, Serialize, Deserialize)]
pub struct GithubGitResponse {
    #[property(description = "The repository the command ran against, as owner/repo")]
    pub repository: String,
    #[property(
        description = "True when git exited 0. False is often the ANSWER rather than a problem — `git push` refusing a non-fast-forward, `git merge` stopping on a conflict, `git commit` finding nothing staged are all git telling you what the state is. Read stderr before treating it as a failure"
    )]
    pub success: bool,
    #[property(description = "git's exit code. 0 is success; 1 usually means 'no', 128 usually means 'cannot'")]
    pub exit_code: i32,
    #[property(description = "What git wrote to standard output — the listing, the diff, the log")]
    pub stdout: String,
    #[property(
        description = "What git wrote to standard error. NOT only errors: progress, 'Switched to branch', and the hint after a refused push all arrive here, so read it even when success is true"
    )]
    pub stderr: String,
    #[property(
        description = "True when the output was longer than the cap and was cut. Anything you concluded from a cut output is a conclusion about part of it — narrow the command with a path, `-n`, or `--stat` and ask again"
    )]
    pub truncated: bool,
}

pub struct GithubGitHandler {
    app: Arc<AppContext>,
}

impl GithubGitHandler {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

impl ToolDefinition for GithubGitHandler {
    const FUNC_NAME: &'static str = "github_git";
    const DESCRIPTION: &'static str = "Run git in a connected repository's working copy — the whole of \
git, not a chosen subset. status, diff, log, add, commit, push, pull, fetch, branch, checkout, stash, \
merge, rebase, reset, show, blame: if git can do it, this runs it.\
\
WHY IT EXISTS: a connected repository is a real clone on this server, and the files under \
`github/<connection>/…` are that working copy. The documents tools READ it and never write it — \
documents_upload, documents_edit, documents_delete and documents_update_path all refuse a `github/` \
path — so everything a repository can be asked comes through here: `git log`, `git diff`, `git show`, \
`git blame` for what happened and when, `git status` for what the working copy actually holds.\
\
THE WORKING COPY IS DISPOSABLE, AND THAT GOVERNS WHAT IS WORTH DOING IN IT. Refresh on a connection \
clones the repository again and REPLACES this folder with the new copy — a commit nobody pushed, a \
stash, a branch of your own all go with the old one, and nothing warns you. So anything you do here that \
is meant to last has to be pushed in the same breath; anything else is a scratch space with an \
unpredictable lifetime. A command running here does hold the folder: a refresh waits for your git to \
finish rather than swapping the ground out from under it.\
\
IT RUNS AT THE ROOT OF THE CLONE, WHICH IS NOT ALWAYS WHERE THE DOCUMENTS PATHS START. A connection \
may name a folder inside the repository, and that folder is the ROOT of what `github/<connection>/…` \
shows — so when it does, a documents path is missing that folder as far as git is concerned: a file you \
edited as `github/specs/design/a.md` is `docs/design/a.md` to git if `specs` was connected at `docs`. \
`git add design/a.md` then fails with 'did not match any files' and `git commit -- design/a.md` commits \
nothing. `git status` is what settles it: it runs over the WHOLE repository and prints paths in git's \
spelling, which is the spelling git accepts — read it after editing and pass back what it printed, \
rather than the documents path. A connection with no folder has no offset and the two are the same.\
\
ONLY GIT. The command must start with `git`; anything else is refused by name. It does not go through a \
shell, so there is no second command to chain onto it — `;` and `&&` are just characters git will not \
understand.\
\
CONFLICTS ARE RESOLVED WITH GIT, NOT WITH THE DOCUMENTS TOOLS. `git pull` stopping on one leaves \
markers in the working tree — documents_get shows them, and nothing here can edit them out. \
`git checkout --ours`/`--theirs` on the paths, or `git merge --abort` to undo the whole attempt, is what \
you have; `git status` lists what is still unmerged.\
\
A COMMIT IS ATTRIBUTED TO WHOEVER YOU SAY. Pass `--author \"Name <email>\"` on the commit when the work \
is a person's; without it the committer is this service, which is honest but tells nobody anything.\
\
PUSHING CHANGES SOMEBODY ELSE'S REPOSITORY. It is a real remote with real branches other people work \
on. Read `git status` and `git diff` before committing, and prefer a branch of your own — \
`git checkout -b <name>` — over pushing straight to main unless you were asked to.\
\
FETCHING AND PUSHING NEED THE CONNECTION'S KEY, which is held in memory and is gone after a restart. \
Everything local — status, diff, log, commit, branch — keeps working without it; a network command \
without one fails saying it could not read a username, and the fix is a person typing the key into the \
connection.";
}

#[async_trait::async_trait]
impl McpToolCall<GithubGitInput, GithubGitResponse> for GithubGitHandler {
    async fn execute_tool_call(
        &self,
        model: GithubGitInput,
    ) -> Result<GithubGitResponse, String> {
        // Parsed and checked BEFORE anything is looked up. A command that is not git is refused for what
        // it is rather than after a project lookup that had nothing to do with why it was refused.
        let args = git::parse_git_command(&model.command)?;

        let connection_name = model.connection.trim();

        let (project_id, connection) = {
            let board = self.app.board.read();
            let project = crate::scripts::resolve_project_by_prefix(&board, &model.project)?;

            let connection = project
                .github_connection(connection_name)
                .ok_or_else(|| {
                    let names: Vec<&str> = project
                        .github_connections
                        .iter()
                        .map(|itm| itm.name.as_str())
                        .collect();

                    match names.is_empty() {
                        true => format!(
                            "{} has no connected repositories — one is set up in the browser, under the project's settings",
                            project.prefix
                        ),
                        false => format!(
                            "{} has no connected repository called '{connection_name}'. It has: {}",
                            project.prefix,
                            names.join(", ")
                        ),
                    }
                })?
                .clone();

            (project.id.clone(), connection)
        };

        let clone_dir = workdir::connection_dir(&self.app.git_repos_path, &project_id, connection_name);

        if !workdir::is_cloned(&clone_dir) {
            let mirror = self.app.github.get_or_pending(&project_id, connection_name);

            return Err(format!(
                "'{connection_name}' has not been cloned yet, so there is no working copy to run git in — its state is '{}'{}. A private repository needs its key typing in before the first clone can happen",
                mirror.state,
                match mirror.error.is_empty() {
                    true => String::new(),
                    false => format!(": {}", mirror.error),
                }
            ));
        }

        let key = self.app.github.key(&project_id, connection_name);

        // **The read side of the folder's lock, for as long as git runs in it.** A refresh replaces this
        // folder with a rename, and doing that under a running `git rebase` is how a working copy ends
        // up half in one clone and half in another. Scoped so the guard is gone before the re-listing
        // below takes one of its own — a swap that has been waiting gets in between the two, which is
        // the right order: the listing then describes the folder that won.
        let output = {
            let workdir_lock = self.app.github.workdir_lock(&project_id, connection_name);
            let _guard = workdir_lock.read().await;

            git::run_git(&clone_dir, &args, key.as_deref()).await?
        };

        // Unconditionally, and whatever the command did. A checkout, a pull, a reset, a stash and a merge
        // all rewrite the working tree, and there is no reading of an exit code that reliably says which
        // ones did — `git checkout` exits 0 having replaced every file, and a failed merge exits 1 having
        // written conflict markers into several. Re-walking the copy is cheap and always right.
        crate::github::relist_connection(&self.app, &project_id, &connection).await;

        Ok(GithubGitResponse {
            repository: connection.full_name(),
            success: output.success(),
            exit_code: output.exit_code,
            stdout: output.stdout,
            stderr: output.stderr,
            truncated: output.truncated,
        })
    }
}
