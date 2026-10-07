use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

/// The first segment every connected repository's path starts with, and the one folder name a real
/// document may not use.
///
/// **A reserved root is what lets a cloned repository be served by the tools that serve the project's
/// own.** An agent calls `documents_list` and sees `github/specs/design/system.md` beside
/// `docs/system.md`, and reads either with `documents_get`. What is under this root is a working copy on
/// disk rather than rows in a table, and that is the whole of the difference: it is READ-ONLY — every
/// tool that writes refuses this root, because a refresh clones the repository again and replaces the
/// folder with it — and the versions are git's, which is why `documents_history` and `documents_restore` name the
/// git command instead of answering. Copying a file OUT of here, into a document of the project's own
/// with an id and a history, is still a sync.
pub const GITHUB_ROOT: &str = "github";

/// The longest a connection's name may be. It is one segment of every path the mirror produces, so it is
/// bounded well under [`crate::documents::MAX_PATH_LEN`] rather than at it.
pub const MAX_CONNECTION_NAME_LEN: usize = 60;

/// How many connections one project may hold.
///
/// A ceiling on work rather than on ambition: every connection is a repository polled on a timer and a tree
/// unpacked on disk, and a project that wanted thirty of them has stopped using this as "the reference
/// material lives over there" and started using it as a build system.
pub const MAX_CONNECTIONS_PER_PROJECT: usize = 8;

/// One connected repository, as a screen and an agent see it.
///
/// **The key is deliberately not here and cannot be.** It never leaves the browser tab it was typed in
/// except on its way to the server, where it is held in memory and nowhere else — so there is no read that
/// could return it, not even to the person who typed it. `has_key` is the whole of what can be said about
/// it: whether the server currently holds one.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GithubConnectionResponse {
    // The connection's name, which is also the folder it appears as: `github/<name>/…`. One path segment,
    // unique within the project.
    pub name: String,
    // The repository, as `owner/repo`. Normalised from whatever url was typed, so two people who pasted
    // different spellings of the same repository see the same thing here.
    pub repo: String,
    // Which branch or tag is mirrored. Empty means the repository's default branch, which is what "I did
    // not say" resolves to and what most connections are.
    pub branch: String,
    // Which folder INSIDE the repository is mirrored, or empty for the whole of it. Everything below is
    // rooted here: `path` of `docs` makes `docs/design/a.md` appear as `github/<name>/design/a.md`.
    pub path: String,
    // Whether the server currently holds a key for this connection. False is not an error — a public
    // repository clones anonymously — and for a private one it stops the exchange with GitHub rather
    // than the folder: a connection that has never cloned shows nothing until a key is given, while one
    // already on disk keeps listing and reading, and only reaching GitHub waits for it.
    pub has_key: bool,
    // What the last pull did. See [`GithubMirrorState`] for the vocabulary; it travels as a string for the
    // same reason every open vocabulary in this contract does.
    pub state: String,
    // What went wrong, when `state` says something did. Empty otherwise. Shown verbatim: "404 — no such
    // repository, or the key cannot see it" is the difference between a typo and a missing key.
    pub error: String,
    // The commit the mirror currently holds, short form. Empty when nothing has been pulled yet.
    pub commit: String,
    // When the mirror was last refreshed, or 0 if never. Unix seconds.
    pub pulled_unix_seconds: i64,
    // How many listings have FINISHED for this connection since the service started — a success and a
    // failure count the same, because what it answers is "has the refresh I asked for happened yet"
    // rather than "did it work". Nothing else can answer that: a pull is asynchronous, and a second
    // `failed` looks exactly like the first one. `/pull` returns the number the connection was at when
    // it accepted the ask, and a HIGHER number here is that run landing. It restarts at 0 with the
    // service, along with every mirror.
    pub pull_no: i64,
    // How many files the mirror holds. Zero with a `ready` state is a real answer: the path names a folder
    // that has nothing in it this side of the filters.
    pub files_amount: i32,
    // How many files the repository holds that the mirror deliberately does not — over the size limit, or
    // at a path this product will not name. A number rather than a list, because it exists to answer one
    // question: is the file I am looking for missing because it was filtered, or because it is not there?
    pub skipped_amount: i32,
}

/// What the last pull did, as the wire spells it.
///
/// Not an enum on the wire — the reader is a screen that draws an unknown value as "unknown" rather than a
/// screen that fails to draw.
pub struct GithubMirrorState;

impl GithubMirrorState {
    /// Nothing has been pulled yet — the connection was just made, or the service just started.
    pub const PENDING: &'static str = "pending";
    /// A pull is running right now.
    pub const PULLING: &'static str = "pulling";
    /// The mirror holds a tree and can be read and synced.
    pub const READY: &'static str = "ready";
    /// The last pull failed, and `error` says how. A mirror that was ready before STAYS readable — a
    /// repository that went unreachable at 3am must not empty the folder somebody is reading at 9.
    pub const FAILED: &'static str = "failed";
    /// A key is needed and the server does not hold one. Its own state rather than a failure, because the
    /// fix is a person typing something rather than anybody debugging anything.
    pub const NEEDS_KEY: &'static str = "needs-key";
}

#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GithubConnectionsResponse {
    pub connections: Vec<GithubConnectionResponse>,
}

/// Create a connection or change one, named by `name`.
///
/// One model for both, the way every other setup call in this product works: a name that is taken is an
/// edit, a name that is free is a new connection.
#[derive(MyHttpInput)]
pub struct SetGithubConnectionInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
    #[http_body(
        name = "name",
        description = "What to call it — one path segment, and the folder it appears as under github/"
    )]
    pub name: String,
    #[http_body(
        name = "url",
        description = "The repository: a github.com url, an owner/repo, or an ssh remote. A /tree/<branch>/<path> url fills the branch and the path in as well"
    )]
    pub url: String,
    #[http_body(
        name = "branch",
        description = "Which branch or tag to mirror. Empty for the repository's default branch"
    )]
    pub branch: Option<String>,
    #[http_body(
        name = "path",
        description = "Which folder inside the repository to mirror. Empty for the whole of it"
    )]
    pub path: Option<String>,
    #[http_body(
        name = "key",
        description = "A GitHub token, for a private repository. Held in memory and never written to the database, so it has to be given again after a restart. Omit to leave whatever key is already held; send an empty string to forget it"
    )]
    pub key: Option<String>,
}

/// Hand the server a key for a connection that already exists — the call a person makes after a restart.
///
/// Separate from [`SetGithubConnectionInputModel`] so that supplying a key is not an edit of the connection:
/// the two are done by different people at different times, and a key dialog that could silently retarget a
/// repository is a key dialog nobody should trust.
#[derive(MyHttpInput)]
pub struct SetGithubKeyInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
    #[http_body(name = "name", description = "Which connection")]
    pub name: String,
    #[http_body(
        name = "key",
        description = "The token. An empty string forgets the one being held"
    )]
    pub key: String,
}

#[derive(MyHttpInput)]
pub struct DeleteGithubConnectionInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
    #[http_body(name = "name", description = "Which connection")]
    pub name: String,
}

#[derive(MyHttpInput)]
pub struct GetGithubConnectionsInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
}

/// Ask for a connection to be pulled now rather than at the next tick of the timer.
#[derive(MyHttpInput)]
pub struct PullGithubConnectionInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
    #[http_body(name = "name", description = "Which connection")]
    pub name: String,
}

/// What asking for a refresh answers — a receipt, not a result.
///
/// **The pull is still running when this comes back**, so what it carries is the one thing that makes the
/// run watchable: where the connection's counter stood the moment the ask was accepted. Read the
/// connections back until [`GithubConnectionResponse::pull_no`] is HIGHER than this, and that is the run
/// finishing — whatever it finished as.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct PullGithubConnectionResponse {
    // The connection's finished-listings count as it was when the refresh was asked for. Watch for a
    // higher one.
    pub pull_no: i64,
}

/// Copy chosen files out of a mirror and into the project's own documents.
///
/// **The mirror is not the documents, and this is the one call that crosses between them.** Everything under
/// `github/` is a working copy of somebody else's repository; what this writes is a document of the
/// project's own, with an id, a version and a history, exactly as an upload would.
#[derive(MyHttpInput)]
pub struct SyncGithubInputModel {
    #[http_body(name = "project", description = "Which project, by prefix")]
    pub project: String,
    #[http_body(name = "connection", description = "Which connection to copy out of")]
    pub connection: String,
    #[http_body(
        name = "paths",
        description = "What to copy, relative to the connection's root. A folder takes everything under it; a file takes itself. Empty takes the whole mirror"
    )]
    pub paths: Vec<String>,
    #[http_body(
        name = "folder",
        description = "Which folder of the project's documents to write into. Empty for the top level"
    )]
    pub folder: Option<String>,
    #[http_body(
        name = "overrideExisting",
        description = "Write everything chosen, replacing documents that are already at those paths with a new version. Off writes only what is not there yet and reports the rest as skipped"
    )]
    pub override_existing: bool,
}

/// Whether a path is inside the reserved root — a document the project does not own.
///
/// The `github` segment alone counts: it is the folder the mirrors hang under, and a real document called
/// `github` would be a file competing with a folder full of them.
pub fn is_github_path(path: &str) -> bool {
    let path = path.trim().trim_start_matches('/');

    path == GITHUB_ROOT
        || path
            .strip_prefix(GITHUB_ROOT)
            .map(|rest| rest.starts_with('/'))
            .unwrap_or(false)
}

/// Where one mirrored file appears in the project's document paths.
pub fn github_mirror_path(connection: &str, relative_path: &str) -> String {
    format!("{GITHUB_ROOT}/{connection}/{relative_path}")
}

/// Split a mirrored path back into the connection it belongs to and the file inside it.
///
/// `None` for anything that is not under the reserved root, and for the root itself — neither names a file.
pub fn parse_github_mirror_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.trim().trim_start_matches('/').strip_prefix(GITHUB_ROOT)?;
    let rest = rest.strip_prefix('/')?;

    let (connection, relative_path) = rest.split_once('/')?;

    if connection.is_empty() || relative_path.is_empty() {
        return None;
    }

    Some((connection, relative_path))
}

/// What every file of a connected repository is named by, on this side.
///
/// **It is derived from where the file is, and it is honest about that.** A document's id is a
/// `SortableId` minted once and never changed, which is what makes its history complete. A file in a
/// repository has none of that: this id changes when the file moves and disappears when the file does.
/// The prefix is what stops it being mistaken for the other kind — nothing that starts with this is ever
/// looked for in Postgres.
///
/// Here in `shared` rather than beside the mirror, because both sides read it now: the server mints these
/// ids into every listing, and the browser has to recognise one to open the file it names.
pub const MIRROR_ID_PREFIX: &str = "github:";

/// The id a file of a connected repository is named by.
///
/// It carries the project because an id is looked up on its own — `documents_get` takes one without a
/// project beside it — and a path is only unique within a board.
pub fn mirror_document_id(project_prefix: &str, mirror_path: &str) -> String {
    format!("{MIRROR_ID_PREFIX}{project_prefix}:{mirror_path}")
}

/// Split such an id back into the project and the path it names.
pub fn parse_mirror_document_id(id: &str) -> Option<(&str, &str)> {
    let rest = id.trim().strip_prefix(MIRROR_ID_PREFIX)?;
    let (project_prefix, mirror_path) = rest.split_once(':')?;

    if project_prefix.is_empty() || mirror_path.is_empty() {
        return None;
    }

    Some((project_prefix, mirror_path))
}

/// Check a connection's name, or say why it is not one.
///
/// It is a folder name in every path the mirror produces, so the rules are the rules a path segment lives
/// by — plus no slash, because a name that could contain one would be a name that could invent a folder
/// level nobody asked for.
pub fn normalise_connection_name(src: &str) -> Result<String, String> {
    let name = src.trim();

    if name.is_empty() {
        return Err("a connection needs a name — it is the folder it appears as".to_string());
    }

    if name.len() > MAX_CONNECTION_NAME_LEN {
        return Err(format!(
            "that name is {} characters — the limit is {MAX_CONNECTION_NAME_LEN}",
            name.len()
        ));
    }

    if name.contains('/') || name.contains('\\') {
        return Err(format!(
            "'{name}' contains a slash — a connection is ONE folder, and the tree inside it comes from the repository"
        ));
    }

    if name == "." || name == ".." {
        return Err(format!("'{name}' is not a name"));
    }

    // Letters and digits in any script, plus the three separators a folder name normally uses. Narrow on
    // purpose: this name is a path segment, a directory component on the container's disk, a segment of a
    // percent-encoded raw url AND part of a document id that is parsed back apart. A `%` or a `#` in it
    // would survive three of those four and break the fourth, which is exactly the kind of bug that only
    // shows up for the one person who named their connection with a symbol.
    if let Some(bad) = name
        .chars()
        .find(|itm| !itm.is_alphanumeric() && !matches!(itm, '-' | '_' | '.'))
    {
        return Err(format!(
            "'{name}' contains '{bad}' — a connection's name is a folder name: letters, digits, '-', '_' and '.'"
        ));
    }

    Ok(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reserved_root_covers_itself_and_everything_under_it() {
        assert!(is_github_path("github"));
        assert!(is_github_path("github/specs/a.md"));
        assert!(is_github_path("/github/specs/a.md"));

        // A folder whose name merely starts with the same letters is a folder of the project's own.
        assert!(!is_github_path("github-notes/a.md"));
        assert!(!is_github_path("docs/github/a.md"));
        assert!(!is_github_path("docs/a.md"));
    }

    #[test]
    fn a_mirrored_path_says_which_connection_it_came_from() {
        assert_eq!(
            parse_github_mirror_path("github/specs/design/a.md"),
            Some(("specs", "design/a.md"))
        );

        // The root and a connection with nothing under it name no file.
        assert_eq!(parse_github_mirror_path("github"), None);
        assert_eq!(parse_github_mirror_path("github/specs"), None);
        assert_eq!(parse_github_mirror_path("docs/a.md"), None);
    }

    #[test]
    fn a_path_survives_the_round_trip() {
        let path = github_mirror_path("specs", "design/a.md");
        assert_eq!(path, "github/specs/design/a.md");
        assert_eq!(
            parse_github_mirror_path(&path),
            Some(("specs", "design/a.md"))
        );
    }

    #[test]
    fn a_connection_name_is_one_folder_and_nothing_else() {
        assert_eq!(normalise_connection_name("  specs "), Ok("specs".to_string()));

        assert!(normalise_connection_name("").is_err());
        assert!(normalise_connection_name("a/b").is_err());
        assert!(normalise_connection_name("a\\b").is_err());
        assert!(normalise_connection_name("..").is_err());
    }

    /// Any script, because a team names things in its own language — but nothing that could change what a
    /// url or an id MEANS once this name is embedded in one.
    #[test]
    fn a_name_may_be_in_any_script_and_hold_no_url_punctuation() {
        assert!(normalise_connection_name("Документация").is_ok());
        assert!(normalise_connection_name("api-specs.v2").is_ok());
        assert!(normalise_connection_name("api_specs").is_ok());

        for bad in ["a%b", "a#b", "a?b", "a b", "a:b", "a&b"] {
            assert!(
                normalise_connection_name(bad).is_err(),
                "'{bad}' should be refused"
            );
        }
    }
}
