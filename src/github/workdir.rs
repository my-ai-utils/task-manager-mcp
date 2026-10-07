//! The working copy of one connected repository: where it lives on disk, how it is brought into
//! existence, and how the files in it are listed, read and written.
//!
//! **A connection is a real clone now, not a window.** What is under `github/<name>/…` is a folder on a
//! mounted volume, produced by `git clone` and kept current by `git fetch`. Reading a file is reading a
//! file; editing one edits it in the working tree, where git can then see it as a change. That is the
//! whole reason the design moved off holding blob references: a reference can be read and cannot be
//! edited, and the point of connecting a repository is to work in it.
//!
//! Two consequences worth stating because everything here depends on them:
//!
//! * **The working copy is a VIEW, and nothing this product offers writes into it.** A connected
//!   repository is read-only on the documents surface: files are listed and read, and the way to change
//!   one is to change it in the repository. That is what makes the folder replaceable, and what
//!   [`reclone_repository`] — Refresh — does with that: it clones into a folder of its own and swaps the
//!   two, so a connection is only ever the old copy or the new one, never a hole in between.
//! * **The key still does not survive a restart, and that is now survivable.** The clone stays; only
//!   reaching GitHub needs the token again. A connection that comes up `needs-key` after a deploy is
//!   still fully readable — it just cannot fetch until somebody types the key.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use ahash::AHashMap;
use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::documents::normalise_document_path;
use tokio::sync::RwLock;

use crate::board::GithubConnectionModel;
use crate::scripts::MAX_BINARY_LEN;

use super::git::{run_git_in_parent, run_git_ok, run_git_uncut};
use super::mirror::{MAX_MIRROR_FILES, MirrorEntry};

/// Where one connection's clone lives, under the configured root.
///
/// By project id and then by connection name, because a name is only unique within a project — two boards
/// may each connect a repository called `specs`, and they are two different connections that must not
/// share a working tree.
///
/// Neither segment can escape the root: a project id is a `SortableId` (digits, hex and dashes) and a
/// connection name has already been through `normalise_connection_name`, which refuses slashes, `..` and
/// everything but letters, digits, `-`, `_` and `.`.
pub fn connection_dir(git_repos_path: &str, project_id: &str, connection_name: &str) -> PathBuf {
    Path::new(git_repos_path)
        .join(project_id)
        .join(connection_name)
}

/// The root of what a connection SHOWS, which is the clone plus the folder inside the repository that was
/// connected.
pub fn connection_root(clone_dir: &Path, repo_path: &str) -> PathBuf {
    if repo_path.is_empty() {
        clone_dir.to_path_buf()
    } else {
        clone_dir.join(repo_path)
    }
}

/// The url git is pointed at.
///
/// https rather than ssh because the credential is a token: an https remote takes one as a header, where
/// an ssh remote would need a key pair written to the volume and an ssh agent to hold it.
pub fn repo_url(connection: &GithubConnectionModel) -> String {
    format!(
        "https://github.com/{}/{}.git",
        connection.owner, connection.repo
    )
}

/// Whether this folder is a clone already.
pub fn is_cloned(clone_dir: &Path) -> bool {
    clone_dir.join(".git").exists()
}

/// Clone the repository if it is not there yet.
///
/// `--branch` only when the connection named one: without it git takes the repository's default branch,
/// which is exactly what an empty branch means and saves asking which one that is.
pub async fn clone_repository(
    clone_dir: &Path,
    connection: &GithubConnectionModel,
    key: Option<&str>,
) -> Result<(), String> {
    let parent = clone_dir
        .parent()
        .ok_or_else(|| format!("'{}' has no parent folder", clone_dir.display()))?;

    let url = repo_url(connection);

    let name = clone_dir
        .file_name()
        .and_then(|itm| itm.to_str())
        .ok_or_else(|| format!("'{}' is not a folder name", clone_dir.display()))?;

    let mut args: Vec<&str> = vec!["clone"];

    if !connection.branch.is_empty() {
        args.push("--branch");
        args.push(&connection.branch);
    }

    args.push(&url);
    args.push(name);

    let output = run_git_in_parent(parent, &args, key).await?;

    if !output.success() {
        // A half-made folder would make `is_cloned` false and every command in it fail on a missing
        // `.git` for ever after. Removed so the next pull tries the clone again from nothing.
        let _ = std::fs::remove_dir_all(clone_dir);

        return Err(output.message());
    }

    Ok(())
}

/// Clone the repository into a folder of its own, and swap it in when it is complete.
///
/// **What the Refresh button does, and it answers a different question from a fetch.** A fetch asks what
/// has changed since; this asks for exactly what GitHub holds now — which is what somebody actually
/// wants when a folder looks wrong: a file a `.gitignore` stopped tracking, a branch that was
/// force-pushed, a working copy still on the branch it was cloned on because the row was edited
/// afterwards. Every one of those survives any number of fetches and none of them survives this.
///
/// **The order is the whole of it: clone FIRST, replace SECOND, and never the other way round.**
///
/// * The clone goes into `<name>~new` beside the connection's folder, and takes as long as a repository
///   takes. Nothing is locked and nothing is deleted for any of it, so the connection carries on being
///   listed and read from the copy it already has — a refresh no longer costs a folder while it runs.
/// * Only when the new clone is complete does the swap happen, and it happens under the write side of
///   `workdir_lock`: `<name>` is renamed to `<name>~old`, `<name>~new` is renamed to `<name>`. Two
///   metadata operations on one filesystem, with every reader of the folder holding the read side, so
///   nobody can be looking at `<name>` at the moment it is neither one copy nor the other.
/// * A clone that FAILS — no key, no network, a repository that is not there — changes nothing at all.
///   The old folder is still the connection's folder, still listed, still readable. That is the
///   difference this staging buys: a refresh can no longer empty a connection by failing.
/// * The old copy is deleted after the lock is released, because deleting a repository's worth of files
///   takes time and nothing is waiting on it.
///
/// What the swap does throw away is anything somebody left in the old folder through `github_git` — an
/// unpushed commit, a stash — which is why the dialog says so before the press.
pub async fn reclone_repository(
    clone_dir: &Path,
    connection: &GithubConnectionModel,
    key: Option<&str>,
    workdir_lock: &RwLock<()>,
) -> Result<(), String> {
    let staged = staging_path(clone_dir, STAGED)?;
    let retired = staging_path(clone_dir, RETIRED)?;

    // Whatever a crash between the two renames left behind, cleared on the way IN rather than trusted: a
    // `~new` from a run nobody is watching is a folder `git clone` would refuse to write into, and a
    // `~old` is a copy of a repository sitting on the disk for no reason.
    remove_staging(&staged)?;
    remove_staging(&retired)?;

    clone_repository(&staged, connection, key).await?;

    let swapped = {
        let _guard = workdir_lock.write().await;

        swap_in(clone_dir, &staged, &retired)
    };

    // Not part of the result: by here the new clone is in place and being read, and a folder that would
    // not delete is disk to reclaim rather than a refresh that failed.
    let _ = remove_staging(&retired);

    swapped
}

/// The two renames, run with the write lock held.
///
/// **Ordered so that a failure leaves a folder rather than a hole.** The old copy is moved aside first;
/// if putting the new one in its place then fails — a disk that filled, a permission that changed — the
/// old one is moved straight back and the connection goes on reading exactly what it read before.
fn swap_in(clone_dir: &Path, staged: &Path, retired: &Path) -> Result<(), String> {
    let had_folder = clone_dir.exists();

    if had_folder {
        std::fs::rename(clone_dir, retired).map_err(|err| {
            format!(
                "the new clone is ready, and '{}' could not be moved aside to make room for it: {err}",
                clone_dir.display()
            )
        })?;
    }

    if let Err(err) = std::fs::rename(staged, clone_dir) {
        // Back where it was. The alternative is a connection with no folder at all, which is the one
        // outcome this whole staging dance exists to prevent.
        if had_folder {
            let _ = std::fs::rename(retired, clone_dir);
        }

        return Err(format!(
            "the new clone could not be moved into place, so the previous one was kept: {err}"
        ));
    }

    Ok(())
}

/// The marker on the folder a re-clone is assembled in, and on the one it replaces.
///
/// **`~` is what makes both safe.** A connection's name is letters, digits, `-`, `_` and `.` and nothing
/// else — `normalise_connection_name` refuses the rest — so no connection can own a folder ending in
/// `~new` or `~old`. The staging folders cannot collide with a working copy, and a delete guarded on the
/// marker cannot reach one.
const STAGED: &str = "~new";
const RETIRED: &str = "~old";

/// One of the two staging folders beside a connection's own.
fn staging_path(clone_dir: &Path, marker: &str) -> Result<PathBuf, String> {
    let name = clone_dir
        .file_name()
        .and_then(|itm| itm.to_str())
        .ok_or_else(|| format!("'{}' is not a folder name", clone_dir.display()))?;

    Ok(clone_dir.with_file_name(format!("{name}{marker}")))
}

/// Delete a staging folder, and refuse to delete anything that is not one.
///
/// **`remove_dir_all` is the most dangerous line in this file, so it is guarded by the NAME it is about
/// to remove** rather than by where the path came from. Every path handed here is built by
/// [`staging_path`], and the check is what survives somebody changing how paths are built: a working
/// copy cannot end in the marker, so this can never be talked into removing one.
///
/// A folder that is not there is not a failure: that is the ordinary state before a first clone.
fn remove_staging(path: &Path) -> Result<(), String> {
    let is_staging = path
        .file_name()
        .and_then(|itm| itm.to_str())
        .map(|itm| itm.ends_with(STAGED) || itm.ends_with(RETIRED))
        .unwrap_or(false);

    if !is_staging {
        return Err(format!(
            "'{}' is not a staging folder — refusing to delete it",
            path.display()
        ));
    }

    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(format!("'{}' did not delete: {err}", path.display())),
    }
}

/// Bring an existing clone up to date WITHOUT ever losing a local change.
///
/// **This is the TIMER's pass, not the button's.** Refresh clones into a folder of its own and swaps it
/// in — see [`reclone_repository`]. What runs every ten minutes has to be cheap over eight repositories
/// that have mostly not moved, so it is a fetch, and it is careful with a working tree that `github_git`
/// may have left something in.
///
/// **The fetch is unlocked and everything after it is not.** `git fetch` writes into `.git` and leaves
/// the working tree alone, so it needs no exclusion and can be the slow half. What follows — reading
/// whether the tree is clean, reading which branch is checked out, and the fast-forward that rewrites
/// files — is one step under the WRITE side of the connection's lock. That is not about two pulls, which
/// cannot overlap anyway: it is about the readers. `documents_get`, a sync of two hundred files, a
/// `github_git` command and the listing walk all hold the READ side, and a read guard excludes nothing
/// against a task that holds no guard at all. Before this took the lock, a ten-minute tick could
/// fast-forward the tree out from under a sync half way through it, and what landed was a folder of
/// documents assembled from two different commits with nothing in the result saying so.
///
/// **Fetch always, merge only when there is nothing to lose.** The timer runs this every ten minutes
/// against a working tree somebody may have run a git command in, so:
///
/// * `git fetch` is unconditional and touches nothing in the working tree;
/// * the fast-forward is attempted only when the tree is clean — no modified file, no untracked file,
///   nothing staged;
/// * only when the branch checked out is the one the fast-forward is FOR;
/// * and it is `--ff-only`, so a branch that has diverged stops rather than opening a merge nobody asked
///   for and leaving conflict markers in files somebody is about to read.
///
/// A refresh that declines to merge is not a failure and is not reported as one: the clone is exactly as
/// usable as it was, and the reason it did not move is a change somebody made. What moves it then is a
/// `git` command the caller runs deliberately — `git stash`, `git commit`, `git pull`, `git merge` — which
/// is the whole point of handing over git rather than a fixed set of buttons.
pub async fn refresh_clone(
    clone_dir: &Path,
    connection: &GithubConnectionModel,
    key: Option<&str>,
    workdir_lock: &RwLock<()>,
) -> Result<(), String> {
    run_git_ok(clone_dir, &["fetch", "--prune", "origin"], key).await?;

    // From here to the end of this function the working tree is this task's alone. The checks below are
    // only worth making if nothing can change the answer between making them and acting on it — and
    // `git status` writes `.git/index` itself, so even the reading half must not race a git command
    // somebody else is running in the same clone.
    let _guard = workdir_lock.write().await;

    if !is_clean(clone_dir).await? {
        return Ok(());
    }

    // Which upstream to fast-forward onto. An unset branch follows whatever the remote's HEAD is, resolved
    // here rather than assumed to be `main` — plenty of repositories are still on `master`, and some are
    // on neither.
    //
    // A clone that does not record `origin/HEAD` is not a failure and must not be reported as one: the
    // fetch above worked, every file is current on disk, and the only thing missing is the name to
    // fast-forward onto. Reported as an error it would put the connection on `failed` with a sentence
    // about a symbolic ref, which describes nothing a reader could act on.
    let upstream = match connection.branch.is_empty() {
        true => match remote_head(clone_dir).await {
            Some(head) => head,
            None => return Ok(()),
        },
        false => format!("origin/{}", connection.branch),
    };

    // **The fast-forward has to land on the branch it is FOR, and git will not check that for us.**
    // `git merge --ff-only origin/dev` run in a working copy sitting on `main` moves MAIN onto dev's tip:
    // it is a merge, and a merge does not care that the two names differ. Two ordinary things reach that
    // state. Somebody edits a connection's branch after it was cloned — the clone is never re-checked-out,
    // so the row says `dev` and the working copy is still on `main`. Or somebody does the thing the
    // `github_git` description recommends and works on a branch of their own, at which point a timer that
    // fast-forwarded it onto the connection's branch would be rewriting their work in the background.
    //
    // Either way nothing would report it: the merge succeeds, the state stays `ready`, and one branch's
    // commits are on another branch's ref. So the names are compared first, and a mismatch declines the
    // merge exactly as a dirty tree does.
    let Some(wanted) = upstream.strip_prefix("origin/") else {
        return Ok(());
    };

    // Detached HEAD answers nothing, which is also a state not to merge into.
    let Some(checked_out) = current_branch(clone_dir).await else {
        return Ok(());
    };

    if checked_out != wanted {
        return Ok(());
    }

    // `--ff-only` failing is the ordinary answer for a branch with local commits on it, not an error to
    // report: the clone stays where it is and the caller can merge or rebase when they choose to.
    let _ = run_git_ok(clone_dir, &["merge", "--ff-only", &upstream], key).await;

    Ok(())
}

/// The branch the working copy has checked out, or `None` on a detached HEAD.
async fn current_branch(clone_dir: &Path) -> Option<String> {
    let branch = run_git_ok(clone_dir, &["symbolic-ref", "--short", "HEAD"], None)
        .await
        .ok()?;

    let branch = branch.trim();

    match branch.is_empty() {
        true => None,
        false => Some(branch.to_string()),
    }
}

/// Whether the working tree has nothing in it that a fast-forward could destroy.
///
/// `--porcelain` is empty exactly when there is nothing staged, nothing modified and nothing untracked
/// that is not ignored — which is the condition a fast-forward is safe under.
async fn is_clean(clone_dir: &Path) -> Result<bool, String> {
    let status = run_git_ok(clone_dir, &["status", "--porcelain"], None).await?;

    Ok(status.trim().is_empty())
}

/// What the remote calls its default branch, as a ref this side can merge, or `None` when the clone does
/// not record one.
///
/// Written by `git clone`, so this is a read of the clone rather than a question for the network — and
/// `None` rather than an error because a clone without it is merely one this cannot fast-forward
/// automatically, not one that is broken.
async fn remote_head(clone_dir: &Path) -> Option<String> {
    let head = run_git_ok(
        clone_dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
        None,
    )
    .await
    .ok()?;

    let head = head.trim();

    match head.is_empty() {
        true => None,
        false => Some(head.to_string()),
    }
}

/// The commit the working copy is on, short form. Empty when the clone has no commits yet.
pub async fn head_commit(clone_dir: &Path) -> String {
    run_git_ok(clone_dir, &["rev-parse", "--short", "HEAD"], None)
        .await
        .map(|itm| itm.trim().to_string())
        .unwrap_or_default()
}

/// Every file the connection shows, read off the working copy.
///
/// **`git ls-files --cached --others --exclude-standard` rather than a directory walk**, and the three
/// flags are the whole of why:
///
/// * `--cached` is what git tracks;
/// * `--others` adds what is in the working tree and not yet tracked — which is how a file just created
///   through `documents_upload` appears in the listing immediately, before anybody commits it;
/// * `--exclude-standard` applies `.gitignore`, so a `target/` or a `node_modules/` somebody built inside
///   the clone does not become forty thousand rows in a documents listing.
///
/// It also never returns anything inside `.git`, which a directory walk would have had to remember to
/// skip.
pub async fn list_working_copy(
    clone_dir: &Path,
    repo_path: &str,
    previous: Arc<Vec<MirrorEntry>>,
    previous_listed: Option<DateTimeAsMicroseconds>,
) -> Result<(Vec<MirrorEntry>, usize), String> {
    // UNCUT, unlike everything a model reads: a listing truncated at 60 000 bytes loses every file past
    // the cut without saying so. See `run_git_uncut`.
    let listed = run_git_uncut(
        clone_dir,
        &["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        None,
    )
    .await?;

    let root = connection_root(clone_dir, repo_path);
    let racy_floor = racy_floor_of(previous_listed);
    let repo_path = repo_path.to_string();

    // **On a blocking thread, and this is the line that made it necessary.** The walk used to be one
    // `stat` per file; it now opens and hashes every text file whose bytes may have moved, which after a
    // re-clone is the whole repository. Thousands of blocking reads on a tokio worker would stall
    // everything else this service is serving — and it holds the connection's read guard while it runs, so
    // the stall would be visible as a refresh that appears to hang.
    tokio::task::spawn_blocking(move || walk_listed(&listed, &root, &repo_path, &previous, racy_floor))
        .await
        .map_err(|err| format!("the listing walk did not finish: {err}"))
}

/// The walk itself: one pass over what git listed, opening only what has to be opened.
///
/// Blocking on purpose — it is called from a blocking thread — and separated from the async shell so that
/// what it needs is exactly what it is given: no lock, no runtime, no borrow of anything that could go
/// away underneath it.
fn walk_listed(
    listed: &[u8],
    root: &Path,
    repo_path: &str,
    previous: &[MirrorEntry],
    racy_floor: i64,
) -> (Vec<MirrorEntry>, usize) {
    // What the last walk knew, by path. Built once rather than searched per file: a repository of five
    // thousand files searched linearly per file is twenty-five million comparisons for nothing.
    let known: AHashMap<&str, &MirrorEntry> = previous
        .iter()
        .map(|itm| (itm.path.as_str(), itm))
        .collect();

    let mut entries: Vec<MirrorEntry> = Vec::new();
    let mut skipped: usize = 0;

    // NUL-separated, which is the only listing git offers that a filename containing a newline cannot lie
    // about — and repositories do contain those. Split on the BYTES: a path is not required to be UTF-8,
    // and one that is not must be skipped rather than take the rest of the repository down with it.
    for path in listed.split(|byte| *byte == 0).filter(|itm| !itm.is_empty()) {
        let path = String::from_utf8_lossy(path);

        // Asked for the size only once the path is known to be in scope, so a repository whose connected
        // folder is one directory of forty does not `stat` the other thirty-nine.
        let Some(relative) = in_scope(&path, repo_path) else {
            continue;
        };

        let full = root.join(&relative);

        // `symlink_metadata` rather than `metadata`: a link is not a file this listing shows, and
        // following one is how a repository would get this service to read something outside the clone.
        // A path git lists and the filesystem does not have is a staged deletion, or a file removed from
        // under us between the listing and this line. Neither is a file to show.
        let Ok(metadata) = std::fs::symlink_metadata(&full) else {
            continue;
        };

        if !metadata.is_file() {
            continue;
        }

        let size = metadata.len() as i64;
        let modified = modified_unix_nanos(&metadata);

        let plan = decide_hash(
            known.get(relative.as_str()).copied(),
            size,
            modified,
            racy_floor,
            binary_by_path(&relative),
        );

        // Reading and hashing is the only part of this walk that opens anything, and it happens for a text
        // file whose (size, moment) pair is not the one already hashed — so a ten-minute tick over a
        // repository nobody has pushed to reads nothing at all, and the pass after a re-clone reads it
        // whole exactly once.
        let (content_hash, force_binary) = match plan {
            HashPlan::Reuse(hash) => (Some(hash), false),
            HashPlan::None => (None, false),
            HashPlan::Read => match std::fs::read(&full) {
                // A file whose extension says text and whose bytes disagree is a file, and is listed as
                // one: `documents_get` would hand it back as bytes, and a brief cannot be written about
                // bytes.
                Ok(bytes) => match crate::documents::is_text_bytes(&bytes) {
                    true => (Some(crate::documents::content_hash(&bytes)), false),
                    false => (None, true),
                },
                // Unreadable now though it was listed a moment ago. Shown without a hash rather than
                // dropped: the file is there, and the next walk will try again.
                Err(_) => (None, false),
            },
        };

        match classify(
            &relative,
            size,
            entries.len(),
            modified,
            content_hash,
            force_binary,
        ) {
            Listed::Entry(entry) => entries.push(entry),
            Listed::Skipped => skipped += 1,
        }
    }

    // Sorted by path, exactly as the documents index is, so everything under one folder is contiguous and
    // a tree can be built by walking the list once.
    entries.sort_by(|left, right| left.path.cmp(&right.path));

    (entries, skipped)
}

/// How long before the last listing a file must have been written for its recorded hash to be trusted.
///
/// **Git's own racy-file rule, for git's own reason.** A filesystem stores mtime at a coarser resolution
/// than a program can write two files, so a file written in the same tick as the listing that recorded it
/// can be changed again afterwards without the pair (size, mtime) moving at all. Anything written that
/// close to the last walk is therefore re-read rather than trusted. Two seconds is generous and costs one
/// extra read of one file.
const RACY_WINDOW_NANOS: i64 = 2_000_000_000;

fn racy_floor_of(previous_listed: Option<DateTimeAsMicroseconds>) -> i64 {
    match previous_listed {
        Some(listed) => listed.unix_microseconds * 1_000 - RACY_WINDOW_NANOS,
        // Nothing has been listed, so nothing is trusted.
        None => i64::MIN,
    }
}

/// When a file was last written, in unix nanoseconds, or 0 when the filesystem will not say.
fn modified_unix_nanos(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|itm| itm.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|itm| itm.as_nanos() as i64)
        .unwrap_or(0)
}

/// What the walk does about one file's hash, decided before anything is opened.
#[derive(Debug, PartialEq)]
enum HashPlan {
    /// The last walk hashed this exact file and nothing about it has moved since.
    Reuse(String),
    /// Open it and hash what is in it.
    Read,
    /// It is not a file this product briefs, so it has no hash at all.
    None,
}

/// Whether the hash the last walk recorded still describes what is on the disk.
///
/// Pure, and separate from the walk, because this is the rule the whole cost of a listing turns on: get it
/// wrong towards reuse and briefs describe a file that has changed; wrong towards reading and every tick
/// reads the whole repository.
fn decide_hash(
    previous: Option<&MirrorEntry>,
    size: i64,
    modified: i64,
    racy_floor: i64,
    is_binary: bool,
) -> HashPlan {
    if is_binary {
        return HashPlan::None;
    }

    let Some(previous) = previous else {
        return HashPlan::Read;
    };

    let Some(hash) = previous.content_hash.as_ref() else {
        return HashPlan::Read;
    };

    // Every one of the three has to hold. The mtime alone is not enough (a write can land in the same
    // tick), the size alone is not enough (an edit that keeps the length), and a file written around the
    // moment of the last walk is not trusted at all.
    let unchanged = previous.size == size && previous.modified_unix_nanos == modified;

    match unchanged && modified < racy_floor {
        true => HashPlan::Reuse(hash.clone()),
        false => HashPlan::Read,
    }
}

/// Whether this product would serve a file at this path as bytes, judged by its name alone.
///
/// **What we CLAIM and what we try to READ are two different questions, and an unknown extension answers
/// them differently.** A file with an unfamiliar extension in a repository is a `.rst`, a `.gradle` or
/// somebody's own suffix far more often than it is a binary, so it is read as text first and the bytes get
/// to disagree.
fn binary_by_path(relative: &str) -> bool {
    match task_manager_shared::documents::content_type_for_path(relative) {
        Some(known) => !crate::documents::is_text_content_type(known),
        None => false,
    }
}

/// What became of one file git listed.
///
/// There is no `Outside` here on purpose: scope is decided by [`in_scope`] before a file is looked at, and
/// a file that was never asked for is not a file that was left out. Only the difference between shown and
/// deliberately-not-shown is worth counting, because only that one answers "is the file I want missing
/// because it was filtered, or because it is not there".
enum Listed {
    Entry(MirrorEntry),
    Skipped,
}

/// Whether a path git listed is inside the connected folder, and what it is called once it is.
fn in_scope(path: &str, repo_path: &str) -> Option<String> {
    let relative = strip_root(path, repo_path)?;

    // The same rule every document path in this product goes through, which is also the sanitiser. A
    // repository path this product would not name is dropped rather than mangled into one it would.
    normalise_document_path(relative).ok()
}

/// Decide whether one in-scope file is shown, and as what.
///
/// Pure, and separate from everything that touches a disk, because this is where the rules that matter
/// live: what is too big to ever be read, how many files one connection may show, and what a browser is
/// told a file is.
///
/// **A file over the single-document limit is skipped rather than listed.** Listing it would put a row in
/// the tree that every read refuses — the honest thing is for it not to be there, with the count saying
/// something was left out.
fn classify(
    relative: &str,
    size: i64,
    shown_so_far: usize,
    modified_unix_nanos: i64,
    content_hash: Option<String>,
    // What the BYTES said, when they were read. The path's own answer is the default; this is how a `.md`
    // that turned out not to be text is listed as the file it is.
    force_binary: bool,
) -> Listed {
    if shown_so_far >= MAX_MIRROR_FILES || size > MAX_BINARY_LEN as i64 {
        return Listed::Skipped;
    }

    let content_type = crate::scripts::content_type_of(None, relative);

    Listed::Entry(MirrorEntry {
        path: relative.to_string(),
        size,
        content_type,
        is_binary: force_binary || binary_by_path(relative),
        content_hash,
        modified_unix_nanos,
    })
}

/// A repository path with the connected folder taken off the front, or `None` when it is not under it.
///
/// The folder somebody connected becomes the ROOT of the tree — that is what makes a path mean what a
/// reader expects when they browse into a repository and copy the address.
fn strip_root<'s>(path: &'s str, repo_path: &str) -> Option<&'s str> {
    if repo_path.is_empty() {
        return Some(path);
    }

    // The slash is what makes `docs` not match `docs2/a.md`.
    path.strip_prefix(repo_path)?.strip_prefix('/')
}

/// Where one file of a connection is on disk, refusing anything that would land outside it.
///
/// **Belt and braces, and both are load-bearing.** `normalise_document_path` has already refused `..` and
/// absolute paths by the time most callers get here — but this is the function that turns a caller's
/// string into a filesystem path, and a check next to the thing it protects is the one that survives a
/// refactor. The second half re-checks the result rather than the input, which is what catches a
/// component that only becomes an escape once the pieces are joined.
pub fn file_path(clone_dir: &Path, repo_path: &str, relative: &str) -> Result<PathBuf, String> {
    let relative = normalise_document_path(relative)?;

    let root = connection_root(clone_dir, repo_path);
    let full = root.join(&relative);

    // Compared against the root as it is spelled, since neither exists yet in the create case and
    // `canonicalize` needs a file that is there.
    if !full.starts_with(&root) {
        return Err(format!(
            "'{relative}' does not name a file inside this connection"
        ));
    }

    Ok(full)
}

/// Read one file of the working copy.
pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|_| "that file is not in the working copy".to_string())?;

    if metadata.len() > MAX_BINARY_LEN as u64 {
        return Err(format!(
            "that file is {} bytes — the limit for one document is {MAX_BINARY_LEN}",
            metadata.len()
        ));
    }

    std::fs::read(path).map_err(|err| format!("that file did not read: {err}"))
}

// There is deliberately no `write_file` and no `delete_file` here. A connected repository is read-only on
// this surface, and the place that rule is worth being unable to break is the module that owns the paths:
// a helper that writes into a working copy is one call away from being reached again.

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(branch: &str, repo_path: &str) -> GithubConnectionModel {
        GithubConnectionModel {
            name: "specs".to_string(),
            owner: "MyJetTools".to_string(),
            repo: "fl-url".to_string(),
            branch: branch.to_string(),
            repo_path: repo_path.to_string(),
        }
    }

    #[test]
    fn a_clone_lives_under_its_project_and_then_its_name() {
        let dir = connection_dir("/root/git-repos", "01HX-abc", "specs");

        assert_eq!(dir, PathBuf::from("/root/git-repos/01HX-abc/specs"));

        // Two boards may each connect something called `specs`, and they are two working trees.
        assert_ne!(dir, connection_dir("/root/git-repos", "01HY-def", "specs"));
    }

    #[test]
    fn the_connected_folder_becomes_the_root() {
        assert_eq!(strip_root("docs/design/a.md", "docs"), Some("design/a.md"));
        assert_eq!(strip_root("docs/a.md", ""), Some("docs/a.md"));

        // A sibling that merely starts with the same letters is not inside it.
        assert_eq!(strip_root("docs2/a.md", "docs"), None);
        assert_eq!(strip_root("src/main.rs", "docs"), None);

        let clone = Path::new("/repos/p/specs");
        assert_eq!(connection_root(clone, ""), clone);
        assert_eq!(
            connection_root(clone, "docs"),
            PathBuf::from("/repos/p/specs/docs")
        );
    }

    /// The function that turns a caller's string into a filesystem path is the one that must not be
    /// talked out of the folder it belongs to.
    #[test]
    fn a_path_that_would_leave_the_connection_is_refused() {
        let clone = Path::new("/repos/p/specs");

        assert_eq!(
            file_path(clone, "docs", "design/a.md").unwrap(),
            PathBuf::from("/repos/p/specs/docs/design/a.md")
        );

        // `..` is refused outright, at any depth and in any position.
        for bad in ["../../../etc/passwd", "a/../../b", "..", "docs/../../x"] {
            assert!(file_path(clone, "docs", bad).is_err(), "{bad}");
        }

        // An ABSOLUTE path is not refused — it is contained, which is the same reading every document
        // path in this product gets: the leading slash is a spelling of the root, and the root is here.
        // Worth a test of its own precisely because "it was not refused" looks alarming until you see
        // where it landed.
        assert_eq!(
            file_path(clone, "docs", "/etc/passwd").unwrap(),
            PathBuf::from("/repos/p/specs/docs/etc/passwd")
        );
    }

    fn entry_of(relative: &str, size: i64) -> Option<MirrorEntry> {
        match classify(relative, size, 0, 0, None, false) {
            Listed::Entry(entry) => Some(entry),
            Listed::Skipped => None,
        }
    }

    /// A path this product will not name is counted rather than mangled into one it would.
    #[test]
    fn a_path_that_is_not_a_document_path_is_out_of_scope() {
        assert_eq!(in_scope("docs/a.md", "docs"), Some("a.md".to_string()));
        assert_eq!(in_scope("../escape.md", ""), None);
        assert_eq!(in_scope("src/main.rs", "docs"), None);
    }

    /// **A page in a repository is more than one file, and the listing is where its types are decided.**
    /// This is the regression test for a mirrored page rendering bare: a stylesheet listed as anything but
    /// `text/css` is refused by the browser.
    #[test]
    fn a_pages_own_assets_are_listed_as_what_they_are() {
        for (path, expected) in [
            ("index.html", "text/html"),
            ("css/design-system.css", "text/css"),
            ("js/design-system.js", "text/javascript"),
            ("build.py", "text/plain"),
            ("img/icon.png", "image/png"),
        ] {
            assert_eq!(entry_of(path, 10).unwrap().content_type, expected, "{path}");
        }
    }

    /// The two questions an unknown extension answers differently: it is OFFERED as bytes, because
    /// claiming a type nobody checked is what broke the page above — and it is still READ as text, because
    /// a file with an unfamiliar suffix in a repository is somebody's own convention far more often than it
    /// is a binary, and the read verifies the bytes anyway.
    #[test]
    fn an_extension_nobody_knows_is_offered_as_bytes_and_still_read_as_text() {
        let unknown = entry_of("notes.unknownext", 10).unwrap();

        assert_eq!(unknown.content_type, "application/octet-stream");
        assert!(!unknown.is_binary, "still read as text first");

        // What the table DOES know is binary stays binary, and is never read as text at all.
        let image = entry_of("logo.png", 10).unwrap();
        assert_eq!(image.content_type, "image/png");
        assert!(image.is_binary);
    }

    /// A file nothing could ever read is not put in the tree — it would be a row every read refuses. The
    /// count is what says something was left out.
    #[test]
    fn a_file_too_big_to_read_is_left_out_and_counted() {
        assert!(entry_of("big.bin", MAX_BINARY_LEN as i64 + 1).is_none());
        assert!(entry_of("fine.md", 4).is_some());

        // And the ceiling on how many one connection shows at all.
        assert!(matches!(
            classify("fine.md", 4, MAX_MIRROR_FILES, 0, None, false),
            Listed::Skipped
        ));
    }

    /// **The two folder names a re-clone uses, and why they cannot be anybody's.** A connection's name
    /// cannot contain `~`, so the staged and retired folders sit beside the working copy in a namespace
    /// no connection can reach — which is also what the delete is guarded on.
    #[test]
    fn a_reclone_stages_beside_the_folder_it_will_replace() {
        let clone = Path::new("/repos/p/specs");

        assert_eq!(
            staging_path(clone, STAGED).unwrap(),
            PathBuf::from("/repos/p/specs~new")
        );
        assert_eq!(
            staging_path(clone, RETIRED).unwrap(),
            PathBuf::from("/repos/p/specs~old")
        );

        // Both are one rename away from the folder they replace — same parent, therefore one filesystem,
        // therefore a rename rather than a copy.
        assert_eq!(
            staging_path(clone, STAGED).unwrap().parent(),
            clone.parent()
        );
    }

    /// The guard on the only `remove_dir_all` in this module. A working copy is not a staging folder and
    /// is refused BY NAME, so a mis-built path cannot be talked into deleting a connection.
    #[test]
    fn only_a_staging_folder_can_be_deleted() {
        assert!(remove_staging(Path::new("/repos/p/specs")).is_err());
        assert!(remove_staging(Path::new("/repos/p")).is_err());
        assert!(remove_staging(Path::new("/")).is_err());

        // A name a connection could actually have, ending in the letters but not the marker.
        assert!(remove_staging(Path::new("/repos/p/specs.new")).is_err());

        // And the two that are this module's own: absent on disk, so removing them is a no-op rather
        // than an error — which is the state a first clone starts from.
        assert!(remove_staging(Path::new("/repos/p/specs~new")).is_ok());
        assert!(remove_staging(Path::new("/repos/p/specs~old")).is_ok());
    }

    fn hashed(path: &str, size: i64, modified: i64) -> MirrorEntry {
        MirrorEntry {
            path: path.to_string(),
            size,
            content_type: "text/markdown".to_string(),
            is_binary: false,
            content_hash: Some(crate::documents::content_hash(b"one")),
            modified_unix_nanos: modified,
        }
    }

    /// **The rule the whole cost of a listing turns on.** Reuse too eagerly and a brief describes a file
    /// that has changed; read too eagerly and every ten-minute tick reads the whole repository.
    #[test]
    fn a_hash_is_reused_only_when_the_file_is_provably_the_one_that_was_hashed() {
        let previous = hashed("a.md", 10, 1_000);
        let floor = 5_000;

        assert_eq!(
            decide_hash(Some(&previous), 10, 1_000, floor, false),
            HashPlan::Reuse(previous.content_hash.clone().unwrap())
        );

        // Any of the three moving is a read: a rewrite that kept the length, a touch that kept the bytes,
        // and a file this walk has never seen.
        assert_eq!(decide_hash(Some(&previous), 11, 1_000, floor, false), HashPlan::Read);
        assert_eq!(decide_hash(Some(&previous), 10, 1_001, floor, false), HashPlan::Read);
        assert_eq!(decide_hash(None, 10, 1_000, floor, false), HashPlan::Read);

        // Listed last time and never hashed — a file that was binary then, or a walk from before hashes
        // existed at all.
        let unhashed = MirrorEntry {
            content_hash: None,
            ..hashed("a.md", 10, 1_000)
        };

        assert_eq!(decide_hash(Some(&unhashed), 10, 1_000, floor, false), HashPlan::Read);

        // A file written around the moment of the last walk is not trusted however well its pair matches:
        // the filesystem's clock is coarser than two writes.
        assert_eq!(decide_hash(Some(&previous), 10, 1_000, 500, false), HashPlan::Read);

        // And a file is never hashed at all — there is no brief to be written about a PNG.
        assert_eq!(decide_hash(Some(&previous), 10, 1_000, floor, true), HashPlan::None);
    }

    /// Nothing listed before means nothing to trust, which must not accidentally read as "everything is
    /// unchanged".
    #[test]
    fn a_connection_that_has_never_been_listed_trusts_nothing() {
        let floor = racy_floor_of(None);
        let previous = hashed("a.md", 10, 1_000);

        assert_eq!(decide_hash(Some(&previous), 10, 1_000, floor, false), HashPlan::Read);

        // And once there IS a listing, the floor sits a window before it.
        let listed = DateTimeAsMicroseconds::new(10_000_000);

        assert_eq!(
            racy_floor_of(Some(listed)),
            10_000_000 * 1_000 - RACY_WINDOW_NANOS
        );
    }

    #[test]
    fn the_remote_is_https_so_the_token_can_be_a_header() {
        let url = repo_url(&connection("main", ""));

        assert_eq!(url, "https://github.com/MyJetTools/fl-url.git");
        assert!(url.starts_with("https://"), "a token is not an ssh key");
    }
}
