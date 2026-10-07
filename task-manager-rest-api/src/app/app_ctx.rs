use std::sync::Arc;

use encryption::aes::AesKey;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::releases::ReleaseResponse;
use task_manager_shared::tasks::TaskResponse;
use task_manager_shared::ws::{BoardSnapshot, ServerWsPayload};

use crate::board::Board;
use crate::documents::DocumentsIndex;
use crate::github::GithubMirrors;
use crate::postgres::{
    ColumnTemplatesRepo, DocumentsRepo, GoalsRepo, KindTemplatesRepo, ProjectMembersRepo,
    ProjectsRepo, ReleasesRepo, TasksRepo, UsersRepo,
};
use crate::settings::SettingsReader;
use crate::subscribers::ProjectSubscribers;

// Reported to MCP clients on `initialize` — `McpMiddleware::new` takes both as `&'static str`.
pub const APP_NAME: &str = env!("CARGO_PKG_NAME");
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `encryption::aes::AesKey` requires — it panics on anything else.
const SESSION_KEY_LEN: usize = 48;

pub struct AppContext {
    // Postgres is where the data is durable. Nothing reads through these repos except the startup
    // load and the write half of `scripts/` — every read serves from memory.
    pub projects_repo: ProjectsRepo,
    pub column_templates_repo: ColumnTemplatesRepo,
    pub kind_templates_repo: KindTemplatesRepo,
    pub goals_repo: GoalsRepo,
    pub releases_repo: ReleasesRepo,
    pub project_members_repo: ProjectMembersRepo,
    pub tasks_repo: TasksRepo,
    pub users_repo: UsersRepo,

    // THE EXCEPTION to the line above: documents are never loaded into memory, so every read of one comes
    // through here rather than from `board`.
    //
    // Deliberate, and the reason is the push protocol. The board is sent whole down a WebSocket on every
    // change; a document is a text somebody opens occasionally. Holding them in `Board` would mean shipping
    // every text to every open screen every time anybody moved a sticker. What DOES travel in the snapshot
    // is the list of document ids on a task or a goal — a handful of short strings, enough to draw a count —
    // and the text is fetched when it is opened.
    pub documents_repo: DocumentsRepo,

    // The state every read actually serves from.
    pub board: Board,

    // Documents, minus their payloads — see `crate::documents::DocumentsIndex`.
    //
    // Its own field rather than a collection inside `board`, and that separation is the point: `board` is
    // what the WebSocket pushes WHOLE to every open screen on every change, and a document index has no
    // business travelling with it. A screen asks for the index over HTTP and is answered from here; the
    // payloads are the only part that goes to Postgres per read.
    pub documents_index: DocumentsIndex,
    // What each document SAYS, by the hash of what is in it — see `crate::documents::BriefsIndex`. Beside
    // the index rather than on it, because a brief belongs to a content and an index entry belongs to a
    // document: two files of two projects with one text share the brief and share nothing else.
    pub briefs: crate::documents::BriefsIndex,

    // Connected GitHub repositories: the file list of each working copy, and the keys that reach GitHub
    // behind them.
    //
    // Beside `documents_index` rather than inside it, because the two are different kinds of thing. That
    // index is the project's own documents, durable in Postgres and merely cached here. This is a listing
    // of what is on disk in a clone — rebuilt by walking the working copy, never authoritative, and
    // thrown away on every start.
    pub github: Arc<GithubMirrors>,

    // Where those clones live. One folder per connection underneath it, and a mounted volume rather than
    // the container's writable layer — a clone holds edits that exist nowhere else until they are
    // committed and pushed.
    //
    // Read once at startup rather than per call, for the same reason as the session key: it is a deploy
    // to change, and re-reading it on every git command would only add work.
    pub git_repos_path: String,

    // The Homes to tell when a board changes.
    pub subscribers: ProjectSubscribers,

    // Encrypts and reads the session token and the OAuth `state`. There is no session store: the token
    // carries the session, so this key is the whole of it.
    pub session_key: AesKey,

    pub settings_reader: Arc<SettingsReader>,
}

impl AppContext {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        // Read once at startup rather than per request: rotating the key is a deploy, and re-deriving it
        // on every call would only add work.
        //
        // The length is checked here rather than left to `AesKey::new`, which panics. Without this the
        // service would start happily on a misconfigured key and die on the first sign-in — the failure
        // would surface hours later, in the one place nobody is watching.
        let session_encryption_key = settings_reader
            .get_settings()
            .await
            .session_encryption_key
            .clone();

        if session_encryption_key.len() != SESSION_KEY_LEN {
            panic!(
                "settings: session_encryption_key must be exactly {SESSION_KEY_LEN} bytes, got {}",
                session_encryption_key.len()
            );
        }

        let session_key = AesKey::new(session_encryption_key.as_bytes());

        // Trailing slashes trimmed here rather than at every join: this is pasted into a settings file by
        // a person, and `/root/git-repos/` and `/root/git-repos` must not produce two different roots.
        let git_repos_path = settings_reader
            .get_settings()
            .await
            .git_repos_path
            .trim()
            .trim_end_matches('/')
            .to_string();

        if git_repos_path.is_empty() {
            panic!("settings: git_repos_path must name a folder to clone connected repositories into");
        }

        Self {
            projects_repo: ProjectsRepo::new(settings_reader.clone()).await,
            column_templates_repo: ColumnTemplatesRepo::new(settings_reader.clone()).await,
            kind_templates_repo: KindTemplatesRepo::new(settings_reader.clone()).await,
            goals_repo: GoalsRepo::new(settings_reader.clone()).await,
            releases_repo: ReleasesRepo::new(settings_reader.clone()).await,
            project_members_repo: ProjectMembersRepo::new(settings_reader.clone()).await,
            tasks_repo: TasksRepo::new(settings_reader.clone()).await,
            users_repo: UsersRepo::new(settings_reader.clone()).await,
            documents_repo: DocumentsRepo::new(settings_reader.clone()).await,
            board: Board::new(),
            documents_index: DocumentsIndex::new(),
            briefs: crate::documents::BriefsIndex::new(),
            github: Arc::new(GithubMirrors::new()),
            git_repos_path,
            subscribers: ProjectSubscribers::new(),
            session_key,
            settings_reader,
        }
    }

    /// Whether this email is an admin, resolved the way every caller needs it: the flag on the user
    /// row **or** membership of the `admins` list in settings.
    ///
    /// The settings list is additive rather than an alternative, which is what makes an empty
    /// database recoverable — no row there says admin, yet those addresses still get in and create
    /// the roster.
    pub async fn is_admin_in_settings(&self, email: &str) -> bool {
        let email = email.trim().to_lowercase();

        self.settings_reader
            .get_settings()
            .await
            .admins
            .iter()
            .any(|itm| itm.trim().to_lowercase() == email)
    }

    /// Push the board to every Home watching it.
    ///
    /// **The board itself, not a signal to go and re-read it.** A re-read empties the screen for as long as
    /// the round trip takes, and what the reader sees in that moment is a spinner where their board was —
    /// on a board that repaints every time an agent touches anything, that is most of the time. A whole
    /// snapshot cannot drift out of step with the server the way a delta could, so this keeps the one
    /// property the invalidation signal was chosen for.
    ///
    /// Called from `scripts/` after the change is in Postgres **and** in memory.
    ///
    /// **The WHOLE project, archived work included.** Not the live board: a goal outlives the archive window
    /// — it can only be closed once every one of its tasks is done, so by then the oldest of them have aged
    /// off — and the Goals screen has to show a goal's list agreeing with the counters beside it, which count
    /// archived work. Sending the live board meant that screen fetching each expanded goal separately and
    /// throwing those lists away on every push, which is both a round trip and a flicker for a protocol whose
    /// whole point is that nothing is re-read.
    ///
    /// So the filtering moves to the client, where the question differs per screen: the board draws the live
    /// window (`task_manager_shared::tasks::is_task_archived`, the same rule as `BoardInner::is_archived`),
    /// and the Goals screen draws the lot. `/api/tasks/v1/list` takes `includeArchived` for the same reason,
    /// so a screen gets the same thing whichever door it came through.
    pub async fn notify_project_changed(&self, project_id: &str) {
        // Built while the board lock is held and sent after it is dropped: `parking_lot`'s guard is `!Send`,
        // so holding one across an `.await` does not compile — which is the compiler enforcing the thing we
        // want anyway, since one stalled socket must not hold up every other reader of the board.
        let prepared = {
            let board = self.board.read();

            board.get_project(project_id).map(|project| {
                let tasks: Vec<TaskResponse> = board
                    .tasks_of_project(&project.id)
                    .iter()
                    .map(|task| crate::mappers::task_to_response(task, &project, &board))
                    .collect();

                // Live goals only — a closed goal leaves the screen once its window has passed, and is
                // reached by searching for its id after that. Their counters are computed here, over the
                // whole history, which the task list above now also carries: one source, and the list and
                // the number beside it can no longer disagree.
                let goals: Vec<GoalResponse> = board
                    .goals_of_project(&project.id)
                    .iter()
                    .filter(|goal| !board.is_goal_archived(goal))
                    .map(|goal| crate::mappers::goal_to_response(goal, &project, &board))
                    .collect();

                // Every live release, newest first — and NOT cut at the archive window the two lists above
                // are measured against. A release does not age off: the list of them is the history, and
                // one record per feature shipped stays a short list for a long time.
                let releases: Vec<ReleaseResponse> = board
                    .releases_of_project(&project.id)
                    .iter()
                    .map(|release| crate::mappers::release_to_response(release, &project, &board))
                    .collect();

                let members: Vec<String> = project.members.iter().cloned().collect();

                // The PREFIX travels, not the id: it is the only name a project has on the wire, and it is
                // what the watching client sent in its `{"watch":…}` — see `ProjectResponse`.
                (project.prefix.clone(), tasks, goals, releases, members)
            })
        };

        let (payload, members) = match prepared {
            Some((prefix, tasks, goals, releases, members)) => (
                ServerWsPayload::board(BoardSnapshot {
                    project: prefix,
                    tasks,
                    goals,
                    releases,
                }),
                members,
            ),
            // No project in memory to build a board from — nor, therefore, a prefix to name it by. The signal
            // still goes out, and it is delivered by what the connection is WATCHING rather than by anything
            // in the payload: the client reads only the presence of this key and re-reads when it sees it,
            // which is also what the whole protocol did before snapshots. The internal id is deliberately not
            // put here instead — it does not cross this boundary.
            None => (ServerWsPayload::project_changed(""), Vec::new()),
        };

        let Ok(payload) = serde_json::to_string(&payload) else {
            return;
        };

        self.subscribers
            .push_to_watchers(project_id, &payload, &members)
            .await;
    }
}
