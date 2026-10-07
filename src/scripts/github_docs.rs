use std::path::PathBuf;

use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::github::{GITHUB_ROOT, github_mirror_path, parse_github_mirror_path};
// Minted here on every listing and read in the browser to open the file one names, so the spelling lives
// in `shared` beside the reference vocabulary that carries it. Re-exported because this module is where
// the rest of the service has always reached for it.
pub use task_manager_shared::github::{mirror_document_id, parse_mirror_document_id};

use crate::app::AppContext;
use crate::board::ProjectModel;
use crate::documents::DocumentIndexEntry;
use crate::github::workdir;
use crate::postgres::DocumentDto;

use super::resolve_project_by_prefix;

/// Who a file of a connected repository is attributed to in a listing.
///
/// Not a person, and deliberately not an email: what a listing can honestly say is "this came from the
/// repository". Who wrote a particular line is a question for `git log`, which knows, and this side does
/// not — nothing here records an author, because nothing here keeps versions.
pub const MIRROR_AUTHOR: &str = "github";

/// Every file of every connection of one project, as index entries indistinguishable in shape from a
/// document's.
///
/// **Shape, not substance.** They sit in the same list because that is what makes a connected repository
/// readable by the tools that already exist — `documents_list` answers with them and `documents_get`
/// reads them — and they are told apart by their path and their id wherever it matters, which is every
/// tool that WRITES: those refuse the reserved root outright. See [`refuse_mirror_write`].
///
/// `version` is 0 on all of them, and that number is a statement: this product keeps no versions of these
/// files. Their history is the repository's, and `git log` is how it is read.
pub fn mirror_index_entries(app: &AppContext, project: &ProjectModel) -> Vec<DocumentIndexEntry> {
    let mut entries = Vec::new();

    for connection in project.github_connections.iter() {
        let mirror = app.github.get_or_pending(&project.id, &connection.name);

        let updated = mirror.listed.unwrap_or_else(|| DateTimeAsMicroseconds::new(0));

        for file in mirror.entries.iter() {
            let path = github_mirror_path(&connection.name, &file.path);

            entries.push(DocumentIndexEntry {
                id: mirror_document_id(&project.prefix, &path),
                project_id: project.id.clone(),
                path,
                content_type: file.content_type.clone(),
                is_binary: file.is_binary,
                size: file.size,
                // Off the listing walk, which hashed the file when it read it — see `MirrorEntry`. A
                // repository's file has no row to carry one, so the walk is the only place it can come
                // from, and `None` here reads exactly as it does for a document: not briefed.
                content_hash: file.content_hash.clone(),
                version: 0,
                created: updated,
                updated,
                updated_by: MIRROR_AUTHOR.to_string(),
            });
        }
    }

    entries
}

/// One file of a connected repository, resolved all the way down to where it is on disk.
pub struct MirrorTarget {
    pub project_id: String,
    pub project_prefix: String,
    /// Which connection it is in — the second half of the key its folder's lock is kept under.
    pub connection_name: String,
    /// The file, relative to the connection's root.
    pub relative: String,
    /// The file as this product names it: `github/<connection>/<file>`.
    pub mirror_path: String,
    /// Where the file is on disk.
    pub full_path: PathBuf,
}

/// Turn a `github/<connection>/<file>` path into everything needed to touch that file.
///
/// The one place the four questions are answered together — which project, which connection, where its
/// clone is, and which file inside it — so that a read and the listing it came from cannot disagree about
/// any of them.
pub fn resolve_mirror_target(
    app: &AppContext,
    project_prefix: &str,
    mirror_path: &str,
) -> Result<MirrorTarget, String> {
    let Some((connection_name, relative)) = parse_github_mirror_path(mirror_path) else {
        return Err(format!(
            "'{mirror_path}' is not a path in a connected repository — those look like `{GITHUB_ROOT}/<connection>/<file>`"
        ));
    };

    let (project_id, project_prefix, connection) = {
        let board = app.board.read();
        let project = resolve_project_by_prefix(&board, project_prefix)?;

        let connection = project
            .github_connection(connection_name)
            .ok_or_else(|| {
                format!("this project has no connected repository called '{connection_name}'")
            })?
            .clone();

        (project.id.clone(), project.prefix.clone(), connection)
    };

    let clone_dir = workdir::connection_dir(&app.git_repos_path, &project_id, connection_name);

    if !workdir::is_cloned(&clone_dir) {
        let mirror = app.github.get_or_pending(&project_id, connection_name);

        return Err(format!(
            "'{connection_name}' has not been cloned yet — its state is '{}'{}",
            mirror.state,
            match mirror.error.is_empty() {
                true => String::new(),
                false => format!(": {}", mirror.error),
            }
        ));
    }

    let full_path = workdir::file_path(&clone_dir, &connection.repo_path, relative)?;

    Ok(MirrorTarget {
        project_id,
        project_prefix,
        connection_name: connection_name.to_string(),
        relative: relative.to_string(),
        mirror_path: mirror_path.to_string(),
        full_path,
    })
}

/// Read one file of a connected repository, off the disk it is on.
///
/// **No network, no rate limit and no key.** This used to be a request to GitHub per read, which is what
/// made a public repository browsable and a sync of two hundred files not: anonymous reads share sixty an
/// hour. A clone spends that budget once, on the clone, and every read afterwards is a file read.
///
/// The payload is decided the same way an uploaded archive decides it — the extension says what the file
/// is, and the bytes get to disagree. A `.md` that is not UTF-8 is served as bytes rather than as
/// mojibake.
pub async fn read_mirror_document(
    app: &AppContext,
    project_prefix: &str,
    mirror_path: &str,
) -> Result<DocumentDto, String> {
    let target = resolve_mirror_target(app, project_prefix, mirror_path)?;

    // **The read side of the folder's lock, held across the open.** A refresh replaces a connection's
    // whole folder with a rename, and this is the half-second in which one could land: the path was
    // resolved a moment ago and the bytes have not been read yet. Holding the lock is what makes the two
    // one operation — the read sees the old folder or the new one, never the instant between them.
    let workdir_lock = app
        .github
        .workdir_lock(&target.project_id, &target.connection_name);

    let _guard = workdir_lock.read().await;

    let bytes = workdir::read_file(&target.full_path)
        .map_err(|err| format!("'{mirror_path}' did not read: {err}"))?;

    Ok(document_of(&target, bytes))
}

/// The one answer every write to a connected repository gets, whatever tool asked for it.
///
/// **One sentence in one place, because five tools reach this and a reader who learns it from
/// `documents_edit` should recognise it from `documents_delete`.** `what` is what the caller was trying
/// to do to the file — written, edited, deleted, renamed — so the refusal names the actual attempt rather
/// than the category.
///
/// **The reason is not a policy, it is what the disk does.** The folder behind a `github/` path is a
/// clone that Refresh deletes and makes again, so a write into it is work that disappears at the next
/// refresh with nothing left to say it was ever there. Refusing is the honest version of that; accepting
/// it and losing it quietly is not.
///
/// Both ways forward are in the message, because a refusal that does not say what to do instead is a
/// refusal a caller retries.
pub fn refuse_mirror_write(path: &str, what: &str) -> String {
    format!(
        "'{path}' is in a connected repository, and this board only READS those — nothing under `{GITHUB_ROOT}/` can be {what}. What is on the disk is a clone, and a refresh deletes it and clones it again, so a change written into it would be gone at the next refresh with nothing to say it had been there. Change the file where it lives, in the repository, and refresh the connection; or take a copy this board owns — sync it into this project's own documents, which are writable, versioned and yours"
    )
}

/// Refuse moving a document of the project's own INTO a connected repository.
///
/// The mirror image of [`refuse_mirror_write`] and a separate sentence on purpose: what is wrong with
/// this one is not only that nothing writes there. A move keeps a document's id, its versions and every
/// reference pointing at it, and none of those can follow it into somebody else's repository — so even if
/// the write happened, what a reader would get is a file appearing in a working copy while a document
/// quietly vanished from the board.
pub fn refuse_move_into_mirror(path: &str) -> Result<(), String> {
    if !task_manager_shared::github::is_github_path(path) {
        return Ok(());
    }

    Err(format!(
        "'{path}' is inside a connected repository, and a document of this project cannot be MOVED there — this board only reads connected repositories, and even if it wrote to one, the document's id, its versions and every reference to it would not survive the trip. Put the file in the repository itself and refresh the connection; the document here is deleted separately, if it should only live in the repository"
    ))
}

/// One file of a working copy, in the shape every read of a document answers in.
///
/// Text only when the extension says text AND the bytes agree. Same rule, and the same reason, as the
/// archive unpack: storing a guess as text is what produces a document nobody can read back.
fn document_of(target: &MirrorTarget, bytes: Vec<u8>) -> DocumentDto {
    let is_binary = match task_manager_shared::documents::content_type_for_path(&target.relative) {
        Some(known) => !crate::documents::is_text_content_type(known),
        None => false,
    };

    // Hashed off the RAW bytes, before the branch below moves them and before anything decides what they
    // are. That is what makes a file of a repository and a document of the project's own with the same
    // text share one brief: both hash the bytes, neither hashes a rendering of them.
    let content_hash = match crate::documents::is_text_bytes(&bytes) && !is_binary {
        true => Some(crate::documents::content_hash(&bytes)),
        false => None,
    };

    let (content, binary_content) = if is_binary {
        (None, Some(bytes))
    } else {
        match String::from_utf8(bytes) {
            Ok(text) if !text.contains('\0') => (Some(text), None),
            Ok(text) => (None, Some(text.into_bytes())),
            Err(err) => (None, Some(err.into_bytes())),
        }
    };

    let content_size = content
        .as_ref()
        .map(|itm| itm.len() as i64)
        .or_else(|| binary_content.as_ref().map(|itm| itm.len() as i64));

    let now = DateTimeAsMicroseconds::now();

    DocumentDto {
        id: mirror_document_id(&target.project_prefix, &target.mirror_path),
        project_id: target.project_id.clone(),
        doc_path: target.mirror_path.clone(),
        content_type: Some(super::content_type_of(None, &target.relative)),
        content,
        binary_content,
        content_size,
        content_hash,
        // Not a version. See `mirror_index_entries`.
        version: 0,
        created: now,
        updated: now,
        updated_by: MIRROR_AUTHOR.to_string(),
    }
}

/// Refuse a question about a mirrored file's past.
///
/// **Nothing is being prevented; the question is being sent where it can be answered.** This product
/// keeps no versions of a file it did not write — but git keeps all of them, and now that the repository
/// is cloned they are one command away rather than on another machine. Saying which command is the whole
/// content of the message.
pub fn refuse_reserved_history(id: &str) -> Result<(), String> {
    let Some((_, mirror_path)) = parse_mirror_document_id(id) else {
        return Ok(());
    };

    let Some((connection, relative)) = parse_github_mirror_path(mirror_path) else {
        return Ok(());
    };

    Err(format!(
        "'{mirror_path}' is a file in a connected repository, and this product keeps no versions of it — git does. Ask git instead, with github_git on '{connection}': `git log --follow -- {relative}` for what happened to it, `git diff -- {relative}` for what is uncommitted, `git show <commit> -- {relative}` for how it read then"
    ))
}

/// Refuse restoring a file that was never in this product's trash.
///
/// The same shape of answer as [`refuse_reserved_history`] and for the same reason: the undo exists, it
/// simply belongs to git.
pub fn refuse_reserved_restore(path_or_id: &str) -> Result<(), String> {
    let mirror_path = match parse_mirror_document_id(path_or_id) {
        Some((_, mirror_path)) => mirror_path,
        None if task_manager_shared::github::is_github_path(path_or_id) => path_or_id,
        None => return Ok(()),
    };

    let hint = match parse_github_mirror_path(mirror_path) {
        Some((connection, relative)) => format!(
            " Ask git instead, with github_git on '{connection}': `git checkout -- {relative}` puts back a file that was deleted or changed since the last commit, and `git restore --source <commit> -- {relative}` takes it from further back"
        ),
        None => String::new(),
    };

    Err(format!(
        "'{mirror_path}' is a file in a connected repository, and this product's trash has never held one — nothing here writes to a connected repository or deletes from one, so there is nothing to restore it from.{hint}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mirrored_id_says_which_project_and_which_path() {
        let id = mirror_document_id("TM", "github/specs/design/a.md");

        assert_eq!(id, "github:TM:github/specs/design/a.md");
        assert_eq!(
            parse_mirror_document_id(&id),
            Some(("TM", "github/specs/design/a.md"))
        );
    }

    /// A real document's id must never be mistaken for a mirrored one — the two are looked up in
    /// completely different places.
    #[test]
    fn a_real_id_is_not_a_mirrored_one() {
        assert_eq!(parse_mirror_document_id("01HXYZ0123456789ABCDEFG"), None);
        // The prefix alone names nothing.
        assert_eq!(parse_mirror_document_id("github:"), None);
        assert_eq!(parse_mirror_document_id("github:TM:"), None);
    }

    /// The refusals that are left are the two that send somebody to git rather than stopping them — so
    /// the command to run has to actually be in the message.
    #[test]
    fn a_question_about_the_past_is_answered_with_the_git_command_that_answers_it() {
        let err = refuse_reserved_history("github:TM:github/specs/design/a.md").unwrap_err();

        assert!(err.contains("git log"), "{err}");
        assert!(err.contains("design/a.md"), "{err}");
        // Named, because a git command has to be run somewhere and there may be several connections.
        assert!(err.contains("specs"), "{err}");

        let err = refuse_reserved_restore("github/specs/design/a.md").unwrap_err();
        assert!(err.contains("git checkout"), "{err}");

        // A document of the project's own goes down the ordinary path, untouched by either.
        assert!(refuse_reserved_history("01HXYZ0123456789ABCDEFG").is_ok());
        assert!(refuse_reserved_restore("docs/a.md").is_ok());
    }

    /// **The refusal every write gets, and what has to survive anybody editing the wording.** A caller
    /// that is told no and not told why retries; a caller told the file is read-only and not told that a
    /// refresh is what erases writes goes looking for a permission to turn on. So: the path, the attempt
    /// that was refused, the reason there is nothing to argue with, and both ways forward.
    #[test]
    fn a_refused_write_says_what_was_refused_and_what_to_do_instead() {
        let err = refuse_mirror_write("github/specs/design/a.md", "edited");

        assert!(err.contains("github/specs/design/a.md"), "{err}");
        assert!(err.contains("edited"), "{err}");
        assert!(err.contains("READS"), "{err}");
        // The reason, which is a fact about the disk rather than a policy.
        assert!(err.contains("refresh"), "{err}");
        // The two ways out: change it where it lives, or take a copy this board owns.
        assert!(err.contains("in the repository"), "{err}");
        assert!(err.contains("sync"), "{err}");

        // The attempt is named rather than categorised, so five tools do not all say "written".
        assert!(refuse_mirror_write("github/specs", "deleted").contains("deleted"));
    }

    /// A move INTO a connection is refused for its own reasons, and they have to still be in there: a
    /// caller reading only "read-only" would go looking for a way to grant the write, when what is
    /// actually wrong is that a document's identity cannot follow it.
    #[test]
    fn a_move_into_a_repository_is_refused_as_a_move_as_well_as_a_write() {
        let err = refuse_move_into_mirror("github/specs/design/a.md").unwrap_err();

        assert!(err.contains("MOVED"), "{err}");
        assert!(err.contains("versions"), "{err}");
        assert!(err.contains("only reads"), "{err}");

        // A path of the project's own is not this function's business.
        assert!(refuse_move_into_mirror("docs/design/a.md").is_ok());
    }
}
