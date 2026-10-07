use task_manager_shared::github::GithubMirrorState;

use crate::app::AppContext;
use crate::github::MirrorEntry;
use crate::github::workdir;
use crate::postgres::DocumentDto;

use super::{NewDocumentContent, SkippedEntry, resolve_project_by_prefix};

/// Copy chosen files out of a connected repository and into the project's own documents.
///
/// **This is the one crossing between the two, and it is a copy rather than a link.** Everything under
/// `github/` is a window: it changes when the repository changes, it has no history here, and nothing on
/// the board may reference it. What this writes is a document of the project's own — an id, a version, an
/// author, a full history — sitting at a path the reader chose, which then goes on living its own life.
/// The next pull will not touch it, and neither will anybody upstream.
///
/// **`override_existing` is the whole of the difference between the two ways people use this.** Off is
/// "bring in what I have not got", which is safe to press repeatedly and never overwrites a document
/// somebody has since edited. On is "make mine match theirs", which writes a new version over every
/// chosen path — keeping ids and history, exactly as re-uploading a file does. Off is the default
/// because the destructive reading of a button pressed by mistake should be the one you have to ask for.
///
/// **Partial by design**, like the archive upload it is modelled on: a file that cannot be written is
/// reported with a reason and the rest still arrive. Refusing forty files because one of them is a
/// half-megabyte binary would make the feature useless for exactly the repositories it is for.
pub async fn sync_github(
    app: &AppContext,
    project_prefix: &str,
    connection_name: &str,
    paths: &[String],
    folder: Option<&str>,
    override_existing: bool,
    who: &str,
) -> Result<(Vec<DocumentDto>, Vec<SkippedEntry>), String> {
    let (project_id, project_prefix, connection) = {
        let board = app.board.read();
        let project = resolve_project_by_prefix(&board, project_prefix)?;

        let connection = project
            .github_connection(connection_name)
            .ok_or_else(|| format!("this project has no connection called '{connection_name}'"))?
            .clone();

        (project.id.clone(), project.prefix.clone(), connection)
    };

    let mirror = app.github.get_or_pending(&project_id, connection_name);

    if mirror.entries.is_empty() {
        return Err(match mirror.state {
            GithubMirrorState::NEEDS_KEY => format!(
                "'{connection_name}' has nothing to sync — it needs a key: {}",
                mirror.error
            ),
            GithubMirrorState::FAILED => format!(
                "'{connection_name}' has nothing to sync — the last pull failed: {}",
                mirror.error
            ),
            _ => format!(
                "'{connection_name}' has nothing to sync yet — it has not finished pulling"
            ),
        });
    }

    let chosen = select_entries(&mirror.entries, paths);

    if chosen.is_empty() {
        return Err(
            "nothing was chosen — pick a file or a folder from the connected repository".to_string(),
        );
    }

    let clone_dir = workdir::connection_dir(&app.git_repos_path, &project_id, connection_name);

    // **One guard for the whole sync, not one per file.** A sync copies a set of files that were chosen
    // together and is meant to land as one version of the repository; a refresh swapping the folder
    // half way through would produce a folder of documents assembled from two different commits, with
    // nothing in the result saying so. Held for as long as the copying takes, which is what delays a
    // refresh rather than corrupting the answer.
    let workdir_lock = app.github.workdir_lock(&project_id, connection_name);

    let _guard = workdir_lock.read().await;

    let mut written: Vec<DocumentDto> = Vec::new();
    let mut skipped: Vec<SkippedEntry> = Vec::new();

    for entry in chosen {
        let destination = join_folder(folder, &entry.path);

        // Asked of the in-memory index rather than of Postgres: it is the same answer, it is already
        // loaded, and this question is asked once per chosen file.
        if !override_existing && document_exists_at(app, &project_id, &destination) {
            skipped.push(SkippedEntry {
                name: entry.path.clone(),
                reason: format!(
                    "'{destination}' is already there — tick Override to replace it with the repository's version"
                ),
            });
            continue;
        }

        // Read straight off the working copy. **This loop used to be where the whole design's cost was
        // felt** — one GitHub request per file, against an hourly budget of sixty anonymously, so a sync
        // of two hundred files reliably hit a rate limit part way through. A clone pays that once, and a
        // sync of the whole repository is now two hundred file reads.
        let bytes = match workdir::file_path(&clone_dir, &connection.repo_path, &entry.path)
            .and_then(|path| workdir::read_file(&path))
        {
            Ok(bytes) => bytes,
            // Still a skip rather than a failed sync: a file that vanished between the listing and this
            // read is one file's problem, and the rest are worth having.
            Err(err) => {
                skipped.push(SkippedEntry {
                    name: entry.path.clone(),
                    reason: err,
                });
                continue;
            }
        };

        // The same text-or-bytes rule the archive upload uses, and for the same reason: a repository of
        // Markdown has to arrive as documents that render and diff, not as a folder of downloads.
        let content = NewDocumentContent {
            body: super::body_for_entry(&destination, bytes),
            content_type: None,
        };

        match super::upload_document(app, &project_prefix, &destination, content, who).await {
            Ok(row) => written.push(row),
            // One file's problem is one file's problem.
            Err(err) => skipped.push(SkippedEntry {
                name: entry.path.clone(),
                reason: err,
            }),
        }
    }

    Ok((written, skipped))
}

/// Which mirrored files a selection names.
///
/// **A folder means everything under it, and an empty selection means the whole mirror.** Those are the
/// two things a checkbox tree has to be able to say, and neither survives being spelled out as a file
/// list by the client: a tree the reader ticked at the top would have to enumerate hundreds of paths,
/// and every one of them would be a chance to disagree with what the server currently holds.
///
/// A path that names nothing is silently no files rather than an error — the mirror may have moved on
/// between the tree being drawn and the button being pressed, which is precisely the case where refusing
/// the whole sync would be the wrong answer.
fn select_entries<'s>(entries: &'s [MirrorEntry], paths: &[String]) -> Vec<&'s MirrorEntry> {
    let wanted: Vec<String> = paths
        .iter()
        .map(|itm| itm.trim().trim_matches('/').to_string())
        .filter(|itm| !itm.is_empty())
        .collect();

    if wanted.is_empty() {
        return entries.iter().collect();
    }

    entries
        .iter()
        .filter(|entry| {
            wanted.iter().any(|itm| {
                entry.path == *itm
                    // A folder. The slash is what makes `docs` not match `docs2/a.md`.
                    || entry.path.starts_with(&format!("{itm}/"))
            })
        })
        .collect()
}

/// A destination folder and a mirrored file's own path into the document's path.
///
/// The file keeps its whole path under the connection's root, which is what makes a synced folder arrive
/// as a folder rather than as a heap of files with their names collided together.
fn join_folder(folder: Option<&str>, entry_path: &str) -> String {
    let folder = folder.unwrap_or_default().trim().trim_matches('/');

    if folder.is_empty() {
        entry_path.to_string()
    } else {
        format!("{folder}/{entry_path}")
    }
}

fn document_exists_at(app: &AppContext, project_id: &str, path: &str) -> bool {
    app.documents_index
        .of_project(project_id)
        .iter()
        .any(|itm| itm.path == path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str) -> MirrorEntry {
        MirrorEntry {
            path: path.to_string(),
            size: 1,
            content_type: "text/markdown".to_string(),
            is_binary: false,
            content_hash: Some(crate::documents::content_hash(path.as_bytes())),
            modified_unix_nanos: 0,
        }
    }

    fn chosen(entries: &[MirrorEntry], paths: &[&str]) -> Vec<String> {
        let paths: Vec<String> = paths.iter().map(|itm| itm.to_string()).collect();

        select_entries(entries, &paths)
            .iter()
            .map(|itm| itm.path.clone())
            .collect()
    }

    /// The two things a checkbox tree has to be able to say.
    #[test]
    fn a_folder_takes_everything_under_it_and_a_file_takes_itself() {
        let entries = [
            entry("readme.md"),
            entry("design/a.md"),
            entry("design/deep/b.md"),
            entry("design2/c.md"),
        ];

        assert_eq!(
            chosen(&entries, &["design"]),
            vec!["design/a.md", "design/deep/b.md"],
            "a sibling that merely starts the same way is not inside it"
        );

        assert_eq!(chosen(&entries, &["readme.md"]), vec!["readme.md"]);

        assert_eq!(
            chosen(&entries, &["readme.md", "design2"]),
            vec!["readme.md", "design2/c.md"]
        );
    }

    /// Nothing chosen is the whole mirror — which is what "sync this connection" means when nobody
    /// ticked anything in particular.
    #[test]
    fn an_empty_selection_is_everything() {
        let entries = [entry("a.md"), entry("b/c.md")];

        assert_eq!(chosen(&entries, &[]), vec!["a.md", "b/c.md"]);
        // Blanks are not a selection either.
        assert_eq!(chosen(&entries, &["", "  ", "/"]), vec!["a.md", "b/c.md"]);
    }

    /// A path that names nothing takes nothing, rather than failing the sync — the mirror can move
    /// between a tree being drawn and a button being pressed.
    #[test]
    fn a_path_that_names_nothing_quietly_takes_nothing() {
        let entries = [entry("a.md")];

        assert!(chosen(&entries, &["gone.md"]).is_empty());
    }

    #[test]
    fn the_destination_keeps_the_tree_the_repository_had() {
        assert_eq!(join_folder(Some("docs"), "design/a.md"), "docs/design/a.md");
        assert_eq!(join_folder(Some("/docs/"), "a.md"), "docs/a.md");
        assert_eq!(join_folder(Some("  "), "a.md"), "a.md");
        assert_eq!(join_folder(None, "design/a.md"), "design/a.md");
    }
}
