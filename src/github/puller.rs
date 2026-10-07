use std::sync::Arc;
use std::time::Duration;

use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::github::GithubMirrorState;

use crate::app::AppContext;
use crate::board::GithubConnectionModel;

use super::mirror::{Mirror, PullGuard};
use super::workdir;

/// How often every connection's working copy is brought up to date.
///
/// Ten minutes is the number this was asked for and it is still the right shape of number, but what it
/// costs has changed: a tick is now a `git fetch`, which over an unchanged repository is one small
/// exchange and no objects. What it buys is that a specification an agent reads is at most ten minutes
/// behind the branch it came from.
pub const PULL_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// What a pull is asked to do with the folder that is already on the disk.
///
/// **The two are not degrees of the same thing — they answer different questions.** A fetch asks what has
/// changed since; a re-clone asks for exactly what GitHub holds now. Nothing but the first is cheap
/// enough to run over every connection every ten minutes, and nothing but the second settles a folder
/// that looks wrong for a reason a fetch cannot reach: a force-push, a file that stopped being tracked, a
/// working copy left on the branch it was cloned on by a row that was edited afterwards.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PullMode {
    /// Fetch into the clone and fast-forward it when there is nothing to lose — the timer's pass. Clones
    /// when there is nothing on disk yet, because that is the only way to have anything to fetch into.
    Fetch,
    /// Clone into a folder of its own and swap it in when it is complete — what Refresh does. The
    /// connection is readable throughout, and a clone that fails changes nothing. See
    /// [`workdir::reclone_repository`].
    Reclone,
}

/// Keep every connection's working copy fresh, for as long as the process runs.
///
/// **Started after the board is loaded and never awaited.** The first pass runs immediately rather than
/// after the first interval, and it is the pass that matters most: on a fresh volume it is the one that
/// clones every connection in the first place.
pub fn run_puller(app: Arc<AppContext>) {
    tokio::spawn(async move {
        loop {
            pull_every_connection(&app).await;
            tokio::time::sleep(PULL_INTERVAL).await;
        }
    });
}

/// One pass over every connection of every project.
///
/// Sequential on purpose. These are clones on one disk against one host, and running eight of them at
/// once would turn a background refresh into a spike of network and IO that the requests being served
/// alongside it would feel.
pub async fn pull_every_connection(app: &Arc<AppContext>) {
    // The list is taken as a snapshot and the lock released: a fetch is a network call, and holding the
    // board's guard across one would stall every reader of it.
    let work: Vec<(String, GithubConnectionModel)> = {
        let board = app.board.read();

        board
            .projects()
            .iter()
            .flat_map(|project| {
                project
                    .github_connections
                    .iter()
                    .map(|connection| (project.id.clone(), connection.clone()))
                    .collect::<Vec<_>>()
            })
            .collect()
    };

    for (project_id, connection) in work {
        // Claimed with `try_`, so a connection somebody is refreshing right now is skipped rather than
        // queued behind: their re-clone is a better answer than this fetch, and the next tick is ten
        // minutes away.
        let Some(guard) = app.github.try_begin_pull(&project_id, &connection.name) else {
            continue;
        };

        // A fetch, never a re-clone: this runs unattended over every connection of every project, and
        // re-cloning eight repositories every ten minutes is a download nobody asked for.
        pull_connection(app, &project_id, &connection, PullMode::Fetch, guard).await;
    }
}

/// Bring one connection's working copy up to date, and re-read what is in it.
///
/// **Two halves that fail independently, and that separation is the whole behaviour worth knowing.**
/// The first half talks to GitHub — what it does there is [`PullMode`]. The second walks the working copy
/// and rebuilds the listing. The second runs whether or not the first worked, because once a clone exists
/// the files are on this disk: GitHub being unreachable, or the key being gone after a restart, stops the
/// exchange with the remote and stops nothing else. The connection reads `needs-key` or `failed` with the
/// reason on it, AND lists every file it has, AND every git command in it keeps working.
///
/// **A re-clone that fails is no different**, which is what the staging in [`workdir::reclone_repository`]
/// buys: the new copy is assembled beside the old one and only swapped in once it is complete, so a
/// refresh that hits an expired key leaves the connection exactly as readable as it was, with the reason
/// on it. The only connection that ends a pass with nothing is one that has never been cloned at all.
///
/// Nothing here returns an error, because there is nobody to return one to: this runs on a timer, and
/// what a failure produces is a state on the mirror that a screen shows and the next pass retries.
pub async fn pull_connection(
    app: &Arc<AppContext>,
    project_id: &str,
    connection: &GithubConnectionModel,
    mode: PullMode,
    // Taken by the CALLER, not here, and held to the end of this function — which is what makes the
    // counter it bumps on the way out mean this run. The timer claims it with `try_begin_pull` and skips
    // a busy connection; a person's refresh waits for it and then hands it over.
    _guard: PullGuard,
) {
    let previous = app.github.get_or_pending(project_id, &connection.name);

    app.github.set_state(
        project_id,
        &connection.name,
        GithubMirrorState::PULLING,
        previous.error.clone(),
    );

    let key = app.github.key(project_id, &connection.name);

    let clone_dir = workdir::connection_dir(&app.git_repos_path, project_id, &connection.name);

    // Taken once and used three times: the re-clone swaps the folder under the write side, the fetch's
    // fast-forward rewrites files under the write side, and the listing below walks the folder under the
    // read side. All three exclude the readers — a document read, a sync, a `github_git` command — which
    // hold the read side and would otherwise be looking at a tree being rewritten under them.
    let workdir_lock = app.github.workdir_lock(project_id, &connection.name);

    let exchange = match (mode, workdir::is_cloned(&clone_dir)) {
        (PullMode::Reclone, _) => {
            workdir::reclone_repository(&clone_dir, connection, key.as_deref(), &workdir_lock).await
        }
        (PullMode::Fetch, true) => {
            workdir::refresh_clone(&clone_dir, connection, key.as_deref(), &workdir_lock).await
        }
        (PullMode::Fetch, false) => {
            workdir::clone_repository(&clone_dir, connection, key.as_deref()).await
        }
    };

    // Nothing on disk and the clone did not happen: there is no working copy to list, so the failure is
    // the whole of what this connection is right now. The listing goes with it — after a re-clone that
    // did not clone there are no files behind those rows, and a listing that outlives its files is a tree
    // whose every row answers "that file is not in the working copy".
    if !workdir::is_cloned(&clone_dir) {
        let message = exchange
            .err()
            .unwrap_or_else(|| "the repository was not cloned".to_string());

        record_failure(app, project_id, &connection.name, &previous, message, false);
        return;
    }

    // Both reads of the folder under one guard: a listing and the commit it is OF must describe the same
    // working copy, and a swap landing between them would put one repository's files beside another's
    // sha.
    let (listing, commit) = {
        let _guard = workdir_lock.read().await;

        // Read again rather than reusing the snapshot taken before the exchange: a re-clone can take
        // minutes, and a `github_git` command that re-listed in the meantime holds hashes this walk would
        // otherwise pay for a second time.
        let latest = app.github.get_or_pending(project_id, &connection.name);

        let listing = workdir::list_working_copy(
            &clone_dir,
            &connection.repo_path,
            latest.entries.clone(),
            latest.listed,
        )
        .await;

        let commit = workdir::head_commit(&clone_dir).await;

        (listing, commit)
    };

    let (entries, skipped_amount) = match listing {
        Ok(listing) => listing,
        Err(err) => {
            // The clone is there and could not be read — a disk problem rather than a network one, and
            // the one case where what the mirror held is better than what this pass produced. The files
            // are still on the disk, so the listing is kept.
            record_failure(app, project_id, &connection.name, &previous, err, true);
            return;
        }
    };

    let error = exchange.err().unwrap_or_default();

    let state = if error.is_empty() {
        GithubMirrorState::READY
    } else if needs_key(&error) {
        GithubMirrorState::NEEDS_KEY
    } else {
        GithubMirrorState::FAILED
    };

    if !error.is_empty() {
        // Said once per failure rather than swallowed: a connection that has been failing for a day is
        // something a log should be able to show, and the screen only shows the latest. The key is never
        // part of the message — it travels as a header and git does not echo it.
        println!("github: {} of project {project_id} — {error}", connection.name);
    }

    app.github.put(
        project_id,
        &connection.name,
        Mirror {
            state,
            error,
            commit,
            listed: Some(DateTimeAsMicroseconds::now()),
            entries: Arc::new(entries),
            skipped_amount,
            // Carried over rather than bumped: the run is counted once, by the guard's `Drop`, after this
            // has been written. Every other branch out of this function carries it the same way.
            pull_no: previous.pull_no,
        },
    );
}

/// Re-read one connection's working copy WITHOUT touching the network.
///
/// **What a git command calls the moment it has finished.** `github_git` is the one thing left that
/// changes a working copy — a checkout, a merge, a reset — and the timer is a ten-minute clock: a listing
/// that took until the next tick to show what `git checkout <branch>` just did would read as a command
/// that silently did nothing. It does not fetch, because there is nothing to ask GitHub: whatever
/// changed, changed on this disk.
pub async fn relist_connection(
    app: &AppContext,
    project_id: &str,
    connection: &GithubConnectionModel,
) {
    let clone_dir = workdir::connection_dir(&app.git_repos_path, project_id, &connection.name);

    let workdir_lock = app.github.workdir_lock(project_id, &connection.name);

    let previous = app.github.get_or_pending(project_id, &connection.name);

    let (listing, commit) = {
        let _guard = workdir_lock.read().await;

        let listing = workdir::list_working_copy(
            &clone_dir,
            &connection.repo_path,
            previous.entries.clone(),
            previous.listed,
        )
        .await;

        let commit = workdir::head_commit(&clone_dir).await;

        (listing, commit)
    };

    let Ok((entries, skipped_amount)) = listing else {
        return;
    };

    let mut mirror = (*previous).clone();
    mirror.entries = Arc::new(entries);
    mirror.skipped_amount = skipped_amount;
    mirror.commit = commit;
    mirror.listed = Some(DateTimeAsMicroseconds::now());

    app.github.put(project_id, &connection.name, mirror);
}

/// Whether what git said is something a key would fix.
///
/// **Read out of git's own words rather than an exit code**, because git exits 128 for everything from a
/// missing key to a repository that is not there. The distinction earns its keep for the same reason it
/// did against the API: a private repository is indistinguishable from one that does not exist, so
/// "needs a key" when the name is genuinely misspelled is a better wrong answer than "no such
/// repository" when the key simply expired — the first sends somebody to look at both.
///
/// `could not read Username` is the one that matters most in practice: it is what `GIT_TERMINAL_PROMPT=0`
/// produces when a private repository is reached with no credential at all, which is every connection
/// after a restart.
fn needs_key(message: &str) -> bool {
    let message = message.to_lowercase();

    [
        "could not read username",
        "could not read password",
        "authentication failed",
        "terminal prompts disabled",
        "repository not found",
        "permission denied",
        "403 forbidden",
        "invalid username or token",
    ]
    .iter()
    .any(|itm| message.contains(itm))
}

/// Record why a refresh did not happen.
///
/// Reached only when there is no working copy to list, or when the disk would not answer — a fetch that
/// failed over an existing clone goes down the ordinary path, because the files are still there and
/// listing them is still the right answer.
///
/// `keep_listing` is whether the files the last good pass found are still on the disk. They are when the
/// clone is there and merely would not read; they are not when there is no folder at all — a first clone
/// that failed, or one somebody removed on the host — and carrying the old rows over then would draw a
/// tree of files that nothing can open.
fn record_failure(
    app: &Arc<AppContext>,
    project_id: &str,
    name: &str,
    previous: &Mirror,
    message: String,
    keep_listing: bool,
) {
    let state = if needs_key(&message) {
        GithubMirrorState::NEEDS_KEY
    } else {
        GithubMirrorState::FAILED
    };

    println!("github: {name} of project {project_id} — {message}");

    let mut mirror = previous.clone();
    mirror.state = state;
    mirror.error = message;

    if !keep_listing {
        mirror.entries = Arc::new(Vec::new());
        mirror.skipped_amount = 0;
        mirror.commit = String::new();
    }

    app.github.put(project_id, name, mirror);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sentence a restart produces on every private connection, and the one that must send somebody
    /// to the key dialog rather than to GitHub to look for a repository that is right there.
    #[test]
    fn what_a_missing_key_sounds_like_is_told_apart_from_what_a_real_failure_does() {
        for message in [
            "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
            "remote: Repository not found.",
            "fatal: Authentication failed for 'https://github.com/o/r.git/'",
        ] {
            assert!(needs_key(message), "{message}");
        }

        for message in [
            "fatal: unable to access 'https://github.com/o/r.git/': Could not resolve host: github.com",
            "error: Your local changes to the following files would be overwritten by merge",
            "fatal: refusing to merge unrelated histories",
        ] {
            assert!(!needs_key(message), "{message}");
        }
    }
}
