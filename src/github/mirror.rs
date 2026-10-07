use std::sync::Arc;

use ahash::AHashMap;
use arc_swap::ArcSwap;
use parking_lot::Mutex;
use rust_extensions::date_time::DateTimeAsMicroseconds;
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard, RwLock};

/// The most files one connection may list.
///
/// A repository is not a documents folder — it has a `node_modules`, a `target`, a history of somebody's
/// build outputs — and a connection listing a hundred thousand of them would be answering
/// `documents_list` with a hundred thousand paths an agent has to read past to find the four documents
/// it wanted. The cap is a statement about what this is for.
pub const MAX_MIRROR_FILES: usize = 5_000;

/// One file of a connection's working copy, as everything above sees it: WHERE it is and WHAT it is,
/// never the bytes.
///
/// **The bytes are on disk, and this is a listing rather than a cache of them.** The clone is the truth;
/// this is what was there the last time the working copy was walked, held so that `documents_list` can be
/// answered without a directory walk per call. A read goes to the file itself.
///
/// There is no id and no version, and that is deliberate: a file in a connected repository is a file in
/// somebody's repository. Its history is git's, reachable with `git log`, and this product keeps no
/// versions of it beside them.
#[derive(Debug, Clone)]
pub struct MirrorEntry {
    /// Relative to the connection's root — so a connection rooted at `docs` reports `design/a.md`, not
    /// `docs/design/a.md`. What the reader chose is the root; what is under it is the tree.
    pub path: String,
    pub size: i64,
    pub content_type: String,
    pub is_binary: bool,
    /// What this file's BRIEF is filed under — sha256 of the bytes on the disk, lowercase hex.
    ///
    /// `None` for a file that is not text (by extension, or because the bytes disagreed with it) and for
    /// one too big to list, neither of which is ever briefed. A file of a repository has no row anywhere,
    /// so this listing is the only place its hash can live — and it is the only stable name such a file
    /// has: its id is derived from its path and changes the moment somebody moves it.
    pub content_hash: Option<String>,
    /// When the file was last written, in unix nanoseconds — the second half of the cache key that keeps
    /// the ten-minute walk cheap.
    ///
    /// **Not shown anywhere and not part of what a mirror MEANS.** It is here so the next walk can tell,
    /// without opening anything, that a file is the one it hashed last time: same size, same moment, same
    /// bytes. A re-clone rewrites every file, so every mtime is new and every hash is paid for again —
    /// which is right, since a re-clone is where the content actually changed.
    pub modified_unix_nanos: i64,
}

/// What one connection currently knows, and how the last attempt to refresh it went.
///
/// **A failed refresh never empties a mirror that worked.** `entries` and `commit` are what the last
/// SUCCESSFUL walk of the working copy left; `state` and `error` are about the last attempt. A repository
/// that goes unreachable overnight is a warning beside a folder somebody can still read — and now that
/// the files are on disk, "still readable" is literal: GitHub being unreachable stops the fetch and stops
/// nothing else. Reading, editing, committing and diffing all keep working; only exchanging with the
/// remote waits.
#[derive(Debug, Clone)]
pub struct Mirror {
    pub state: &'static str,
    pub error: String,
    pub commit: String,
    pub listed: Option<DateTimeAsMicroseconds>,
    pub entries: Arc<Vec<MirrorEntry>>,
    /// How many files the repository holds that the listing deliberately does not — over the
    /// single-document size limit, or at a path this product will not name. Counted rather than listed:
    /// it is the difference between "that file is not there" and "that file is not there FOR US", and
    /// one number answers it.
    pub skipped_amount: usize,
    /// How many listings have FINISHED for this connection since the service started, whatever they
    /// finished as.
    ///
    /// **It exists so that a refresh somebody pressed can be watched to its end.** Every other field
    /// describes what the mirror holds, and none of them can say whether a particular run is over: a
    /// pull that finds nothing changed leaves every one of them exactly as it was, and a second failure
    /// looks precisely like the first. A number that only goes up says it in one comparison.
    pub pull_no: u64,
}

impl Mirror {
    fn empty(state: &'static str) -> Self {
        Self {
            state,
            error: String::new(),
            commit: String::new(),
            listed: None,
            entries: Arc::new(Vec::new()),
            skipped_amount: 0,
            pull_no: 0,
        }
    }

}

/// Every connection's file list, plus the keys that fetch behind them.
///
/// **Nothing here survives a restart, deliberately.** The connections do — they are rows on a project —
/// but a key is held only in this process's memory, and the listings are rebuilt by the first tick of
/// the timer. That first tick is cheap precisely because nothing is downloaded: it is one small request
/// per connection.
pub struct GithubMirrors {
    // Tokens, by connection. A `Mutex` rather than an `ArcSwap` because this is written as often as it
    // is read and is never read on a hot path — and because a key must not be cloned into a snapshot
    // that outlives the moment it is used.
    keys: Mutex<AHashMap<String, String>>,
    mirrors: ArcSwap<AHashMap<String, Arc<Mirror>>>,
    // One lock per connection over PULLING it — see `try_begin_pull` and `begin_pull`. An async mutex
    // rather than a flag in a set, because the two askers want opposite things from a busy connection:
    // the timer wants to skip it, and a person who pressed Refresh wants to wait for it.
    pulls: Mutex<AHashMap<String, Arc<AsyncMutex<()>>>>,
    // One lock per connection over its FOLDER on the disk — see `workdir_lock`. A `parking_lot::Mutex`
    // guards the map itself because the map is only ever looked in; what is handed out of it is a tokio
    // lock, which is the one that is held across an await.
    workdirs: Mutex<AHashMap<String, Arc<RwLock<()>>>>,
}

/// Held for the length of one pull, and the reason a second one does not start beside it.
///
/// A guard rather than a flag on the mirror because a pull that panics still has to release it, and a
/// `Drop` is the only release that cannot be forgotten.
///
/// **It owns everything it needs, and that is what lets a refresh be handed to a spawned task.** The
/// claim is an owned tokio guard and the registry is an `Arc`, so a caller can take the claim, read the
/// mirror's counter, and only then move the guard into `tokio::spawn` — which is exactly the order that
/// makes the receipt a person watches mean this run rather than the one that was already going.
pub struct PullGuard {
    mirrors: Arc<GithubMirrors>,
    key: String,
    // Dropped after the count below, which is the order that matters: the number a watcher is waiting for
    // moves while the claim is still held, so nothing can start a pull, finish it and bump the same
    // number in between.
    _claim: OwnedMutexGuard<()>,
}

impl Drop for PullGuard {
    /// Release the claim, and count the run.
    ///
    /// **Counted here rather than at the three places a pull can end** — a fresh listing, a commit that
    /// had not moved, a failure — for the same reason the guard exists at all: one of them is easy to
    /// forget and a panic goes through none of them, and an uncounted run is a dialog that watches for
    /// an end that never arrives. It runs after whatever the pull last wrote, so what it bumps is the
    /// number on the mirror that pull produced.
    fn drop(&mut self) {
        self.mirrors.count_finished_pull(&self.key);
    }
}

impl Default for GithubMirrors {
    fn default() -> Self {
        Self::new()
    }
}

impl GithubMirrors {
    pub fn new() -> Self {
        Self {
            keys: Mutex::new(AHashMap::new()),
            mirrors: ArcSwap::from_pointee(AHashMap::new()),
            pulls: Mutex::new(AHashMap::new()),
            workdirs: Mutex::new(AHashMap::new()),
        }
    }

    /// Claim a connection for one pull, or `None` when one is already running — **the timer's ask.**
    ///
    /// `None` is not an error: a connection somebody is refreshing right now does not also need the
    /// ten-minute fetch, and the run already going is the better answer.
    pub fn try_begin_pull(self: &Arc<Self>, project_id: &str, name: &str) -> Option<PullGuard> {
        let key = key_of(project_id, name);

        let claim = self.pull_lock(&key).try_lock_owned().ok()?;

        Some(PullGuard {
            mirrors: self.clone(),
            key,
            _claim: claim,
        })
    }

    /// Claim a connection for one pull, WAITING for whatever is running — **a person's ask.**
    ///
    /// **The two asks are not the same and must not share a door.** A refresh is a re-clone: it answers a
    /// question a fetch cannot, so a Refresh that arrives while the timer is fetching and quietly does
    /// nothing is a button that lies — the fetch finishes, the counter moves, and the screen reports the
    /// re-clone landed. Waiting is what makes the press mean what it says.
    ///
    /// Bounded, because this is called from a request somebody is watching: a fetch is a second or two,
    /// but the pull already running might be a first clone of a large repository, and a browser tab
    /// hanging on that is worse than being told to come back. The error names the state rather than
    /// blaming the caller.
    pub async fn begin_pull_within(
        self: &Arc<Self>,
        project_id: &str,
        name: &str,
        wait: std::time::Duration,
    ) -> Result<PullGuard, String> {
        let key = key_of(project_id, name);
        let lock = self.pull_lock(&key);

        let claim = tokio::time::timeout(wait, lock.lock_owned())
            .await
            .map_err(|_| {
                format!(
                    "'{name}' is already being read — a pull of it has been running for over {} seconds, which a first clone of a large repository can be. Watch it in the connections list and ask again once it has finished",
                    wait.as_secs()
                )
            })?;

        Ok(PullGuard {
            mirrors: self.clone(),
            key,
            _claim: claim,
        })
    }

    /// The lock over pulling one connection, made on first use.
    fn pull_lock(&self, key: &str) -> Arc<AsyncMutex<()>> {
        self.pulls
            .lock()
            .entry(key.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }

    /// The lock over one connection's FOLDER on the disk, as opposed to over the listing above it.
    ///
    /// **Read to touch the files, write to replace the folder — and the write side is held for two
    /// renames and nothing else.** A refresh clones into a folder of its own with nothing locked (that is
    /// the part that takes minutes) and takes this only to swap the two, so what the exclusion actually
    /// covers is the microsecond in which a connection's folder is neither the old copy nor the new one.
    /// Everything that reads the working copy — a document read, a listing walk, a `github_git` command,
    /// a sync — holds the read side for as long as it is looking, which is why a git command that takes
    /// five minutes delays a swap by five minutes rather than having the ground taken from under it.
    ///
    /// **One lock per connection, not one for all of them.** Two boards refreshing two repositories have
    /// nothing to say to each other, and a single lock would make the slowest git command in the product
    /// everybody's problem.
    ///
    /// Handed out as an `Arc` so the caller holds the lock itself rather than a borrow of the map: the
    /// guard is held across awaits, and the map's own mutex must be free the whole time.
    pub fn workdir_lock(&self, project_id: &str, name: &str) -> Arc<RwLock<()>> {
        let key = key_of(project_id, name);

        self.workdirs
            .lock()
            .entry(key)
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone()
    }

    pub fn get(&self, project_id: &str, name: &str) -> Option<Arc<Mirror>> {
        self.mirrors.load().get(&key_of(project_id, name)).cloned()
    }

    /// The mirror for a connection, or an empty one saying nothing has been listed yet.
    ///
    /// A caller drawing a tree wants a mirror either way — "connected, nothing yet" is a state to draw,
    /// not an absence to branch on.
    pub fn get_or_pending(&self, project_id: &str, name: &str) -> Arc<Mirror> {
        match self.get(project_id, name) {
            Some(mirror) => mirror,
            None => Arc::new(Mirror::empty(
                task_manager_shared::github::GithubMirrorState::PENDING,
            )),
        }
    }

    pub fn put(&self, project_id: &str, name: &str, mirror: Mirror) {
        self.update(|map| {
            map.insert(key_of(project_id, name), Arc::new(mirror));
        });
    }

    /// Record what a listing is doing without touching what it has already delivered.
    ///
    /// This is what keeps a failure from emptying a folder: the entries, the commit and the moment of
    /// the last success are carried over from whatever was there.
    pub fn set_state(&self, project_id: &str, name: &str, state: &'static str, error: String) {
        let previous = self.get_or_pending(project_id, name);

        let mut mirror = (*previous).clone();
        mirror.state = state;
        mirror.error = error;

        self.put(project_id, name, mirror);
    }

    /// Forget a connection entirely — its listing, and its key.
    ///
    /// Both, because this is called when somebody deletes the connection: leaving the key behind would
    /// mean a connection re-created under the same name silently inherits a credential nobody re-entered.
    pub fn forget(&self, project_id: &str, name: &str) {
        self.update(|map| {
            map.remove(&key_of(project_id, name));
        });

        self.keys.lock().remove(&key_of(project_id, name));

        // **The two locks are deliberately NOT dropped here.** Detaching a connection that something is
        // mid-pull or mid-swap on would hand the next connection of the same name a brand new lock, and
        // two operations on one folder would then exclude nobody. They are one small entry per connection
        // this process has ever seen, which is a bounded leak and a cheap one — `MAX_CONNECTIONS_PER_PROJECT`
        // is 8.
    }

    /// Hold a key for a connection, or forget the one being held when it is empty.
    pub fn set_key(&self, project_id: &str, name: &str, key: &str) {
        let key = key.trim();
        let map_key = key_of(project_id, name);

        let mut keys = self.keys.lock();

        if key.is_empty() {
            keys.remove(&map_key);
        } else {
            keys.insert(map_key, key.to_string());
        }
    }

    /// The key for a connection, cloned for the duration of one call.
    ///
    /// Cloned rather than handed out behind the lock so a slow request cannot hold every other
    /// connection's key store — and never stored anywhere by the caller.
    pub fn key(&self, project_id: &str, name: &str) -> Option<String> {
        self.keys.lock().get(&key_of(project_id, name)).cloned()
    }

    pub fn has_key(&self, project_id: &str, name: &str) -> bool {
        self.keys.lock().contains_key(&key_of(project_id, name))
    }

    /// Add one to a connection's finished-listings count. See [`PullGuard::drop`], which is its only
    /// caller.
    ///
    /// A connection detached while its listing was running has nothing to count, and nothing is put back
    /// for it: the row is gone, and re-creating an entry here would resurrect a mirror nobody owns.
    fn count_finished_pull(&self, key: &str) {
        self.update(|map| {
            let Some(existing) = map.get(key) else {
                return;
            };

            let mut mirror = (**existing).clone();
            mirror.pull_no += 1;

            map.insert(key.to_string(), Arc::new(mirror));
        });
    }

    fn update(&self, apply: impl FnOnce(&mut AHashMap<String, Arc<Mirror>>)) {
        // Read-copy-update. The map is small — one entry per connection across the whole product — and is
        // read on every documents listing, which is the shape `ArcSwap` is for.
        let mut next = (**self.mirrors.load()).clone();
        apply(&mut next);
        self.mirrors.store(Arc::new(next));
    }
}

fn key_of(project_id: &str, name: &str) -> String {
    // A newline separates them because neither a project id nor a connection name can contain one, so
    // no pair of (project, name) can collide with another.
    format!("{project_id}\n{name}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number a screen watching a refresh compares against: it moves once, when the run ENDS, and it
    /// moves whatever the run did.
    #[test]
    fn a_listing_is_counted_when_it_ends_and_not_before() {
        let mirrors = Arc::new(GithubMirrors::new());

        assert_eq!(mirrors.get_or_pending("P", "specs").pull_no, 0);

        {
            let _guard = mirrors
                .try_begin_pull("P", "specs")
                .expect("nothing running");

            // What a pull does first, and what a failing one does last — neither is the end of the run.
            mirrors.set_state(
                "P",
                "specs",
                task_manager_shared::github::GithubMirrorState::PULLING,
                String::new(),
            );

            assert!(
                mirrors.try_begin_pull("P", "specs").is_none(),
                "a second listing must not start while one is claimed"
            );

            assert_eq!(
                mirrors.get_or_pending("P", "specs").pull_no,
                0,
                "still running — nothing to report yet"
            );
        }

        assert_eq!(mirrors.get_or_pending("P", "specs").pull_no, 1);

        // And the claim is free again, which is what makes a second refresh possible at all.
        let _second = mirrors.try_begin_pull("P", "specs").expect("released");
    }

    /// **The lock is per connection and stable for as long as the connection is**, which is the whole of
    /// what makes it a lock at all: two callers that looked it up separately must get the same one, or
    /// the swap excludes nobody.
    #[tokio::test]
    async fn two_callers_asking_for_one_connection_get_one_lock() {
        let mirrors = Arc::new(GithubMirrors::new());

        let first = mirrors.workdir_lock("P", "specs");
        let second = mirrors.workdir_lock("P", "specs");

        assert!(Arc::ptr_eq(&first, &second));

        // Two readers of one folder do not exclude each other — a listing and a document read run at the
        // same time all day.
        let _read = first.read().await;
        assert!(second.try_read().is_ok());

        // A swap does. This is the assertion the whole design rests on.
        assert!(second.try_write().is_err());

        // And another connection's folder is another lock: one repository's five-minute git command is
        // not every other repository's problem.
        let other = mirrors.workdir_lock("P", "api");
        assert!(!Arc::ptr_eq(&first, &other));
        assert!(other.try_write().is_ok());
    }

    /// A connection detached mid-listing is not re-created by the run ending.
    #[test]
    fn a_listing_that_outlived_its_connection_counts_nothing() {
        let mirrors = Arc::new(GithubMirrors::new());

        {
            let _guard = mirrors
                .try_begin_pull("P", "specs")
                .expect("nothing running");
            mirrors.forget("P", "specs");
        }

        assert!(mirrors.get("P", "specs").is_none());
    }

    /// **The two ways of asking for a connection want opposite things from a busy one, and this is the
    /// regression test for a refresh that lied.** The timer skips; a person waits. When they collided
    /// before, the refresh was simply dropped — and the fetch that was running bumped the counter the
    /// screen was watching, so it reported a re-clone that never happened.
    #[tokio::test]
    async fn the_timer_skips_a_busy_connection_and_a_person_waits_for_it() {
        let mirrors = Arc::new(GithubMirrors::new());

        let running = mirrors.try_begin_pull("P", "specs").expect("nothing running");

        // What a pull writes the moment it starts. It matters here for the same reason it matters in
        // production: the count rides on the mirror, and a connection nothing has ever written for has
        // no mirror to count on — see `count_finished_pull`.
        mirrors.set_state(
            "P",
            "specs",
            task_manager_shared::github::GithubMirrorState::PULLING,
            String::new(),
        );

        // The timer's ask, refused at once rather than queued.
        assert!(mirrors.try_begin_pull("P", "specs").is_none());

        // A person's ask, while the same pull is still held: it must not come back yet.
        let waiting = mirrors.clone();

        let queued = tokio::spawn(async move {
            waiting
                .begin_pull_within("P", "specs", std::time::Duration::from_secs(5))
                .await
        });

        tokio::task::yield_now().await;
        assert!(!queued.is_finished(), "a person's ask waits for the run in flight");

        // Releasing the run in flight is what lets it through — and the counter has moved exactly once,
        // for the run that ended, before the next one starts.
        drop(running);

        let guard = queued.await.expect("the task ran").expect("the claim came free");

        assert_eq!(mirrors.get_or_pending("P", "specs").pull_no, 1);

        // And the connection is claimed again, so the timer arriving now is refused in its turn.
        assert!(mirrors.try_begin_pull("P", "specs").is_none());

        drop(guard);
        assert_eq!(mirrors.get_or_pending("P", "specs").pull_no, 2);
    }

    /// Waiting is bounded, because the caller is a request somebody is watching. The message says what
    /// is happening rather than that the caller did something wrong.
    #[tokio::test]
    async fn waiting_for_a_pull_that_never_ends_gives_up_and_says_so() {
        let mirrors = Arc::new(GithubMirrors::new());

        let _running = mirrors.try_begin_pull("P", "specs").expect("nothing running");

        // Matched rather than `expect_err`: a claim is not a printable thing, and making it one only to
        // fail a test would put a `Debug` on a guard that nothing else wants.
        let err = match mirrors
            .begin_pull_within("P", "specs", std::time::Duration::from_millis(20))
            .await
        {
            Ok(_) => panic!("the claim was free while something was holding it"),
            Err(err) => err,
        };

        assert!(err.contains("specs"), "{err}");
        assert!(err.contains("already being read"), "{err}");

        // Nothing was counted: no run started, so no run ended.
        assert_eq!(mirrors.get_or_pending("P", "specs").pull_no, 0);
    }

    /// One connection's pull does not hold another's up.
    #[tokio::test]
    async fn a_pull_of_one_connection_leaves_the_others_free() {
        let mirrors = Arc::new(GithubMirrors::new());

        let _specs = mirrors.try_begin_pull("P", "specs").expect("nothing running");

        assert!(mirrors.try_begin_pull("P", "api").is_some());
        assert!(mirrors.try_begin_pull("Q", "specs").is_some());
    }
}
