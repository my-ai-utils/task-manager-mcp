use std::time::Duration;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::github::{GithubConnectionResponse, GithubMirrorState};

/// How often the watch asks whether the refresh is over.
///
/// A second, and it is cheap on both sides: the answer is served out of the service's memory and never
/// touches GitHub. What is being waited on is a `git clone` of the whole repository — a refresh clones
/// beside the folder it will replace — so the wait is measured in tens of seconds rather than in ticks.
const POLL_EVERY: Duration = Duration::from_secs(1);

/// How long the watch keeps asking before it says so out loud.
///
/// **Long enough to outlast the thing being watched, which is now a clone every time.** A refresh deletes
/// the working copy and clones the repository again, so the two-and-a-half minutes that comfortably
/// covered a fetch would have given up on half the repositories this is used on. Git's own ceiling for
/// one command is 300 seconds; the watch matches it, so what it reports is git's answer rather than the
/// dialog's impatience.
///
/// Giving up is still said as the watching stopping rather than as the refresh failing: a dialog that
/// spins for ever has stopped telling the truth, and one that reports a failure that did not happen is
/// worse.
const GIVE_UP_AFTER: Duration = Duration::from_secs(300);

/// One connection's refresh, and the number it is being watched against.
///
/// **A refresh has no id, and this is what stands in for one.** The call that asks for it comes back
/// before the listing has run, states repeat — a second `failed` looks exactly like the first — and the
/// mirror of a repository nobody has touched comes out of a pull identical to how it went in. What cannot
/// repeat is the connection's count of FINISHED listings: the ask returns where it stood, and a higher one
/// coming back is this run ending.
#[derive(Clone, PartialEq)]
pub struct GithubRefreshRun {
    pub connection: String,
    pub pull_no: i64,
}

/// Why the watch stopped. `None` while it is still going, and before there is anything to watch.
#[derive(Clone, PartialEq)]
enum Stopped {
    /// Every run being watched has ended — whatever each of them ended as.
    Landed,
    /// It waited long enough. Said as "the watching stopped", never as "the listing stopped", because the
    /// listing is somewhere else and this side does not know.
    GaveUp,
    /// The connections could not be read back. About the watch, not about the repository.
    Failed(String),
}

#[derive(Default)]
struct ComponentState {
    /// The connections this dialog is about, as the last answer described them.
    ///
    /// Loaded when the dialog opens and replaced by every tick of the watch, so the rows are read from
    /// ONE place whether or not anything is running. A refresh that changes nothing then redraws as the
    /// same screen with the same numbers, which is the honest picture of what happened.
    connections: DataState<Vec<GithubConnectionResponse>>,
    stopped: Option<Stopped>,
    /// Which watch is the current one.
    ///
    /// Pressing Refresh again while the previous watch is still asking leaves two loops running against
    /// the same state, and the older one is watching for a number that has already gone past. Its answers
    /// must not land on the screen and it must not report anything back.
    generation: usize,
}

impl ComponentState {
    /// Claim the state for a new ask, and say which watch it is.
    ///
    /// Called twice per refresh, and both are the point: once when the button is pressed, so the screen
    /// stops saying how the PREVIOUS run ended while a new one is being asked for, and once when the runs
    /// come back, which is the watch itself starting.
    fn begin_watch(&mut self) -> usize {
        self.generation += 1;
        self.stopped = None;
        self.generation
    }

    fn is_current(&self, generation: usize) -> bool {
        self.generation == generation
    }

    /// What one tick of the watch saw.
    fn set_polled(&mut self, connections: Vec<GithubConnectionResponse>, landed: bool) {
        self.connections.set_loaded(connections);

        if landed {
            self.stopped = Some(Stopped::Landed);
        }
    }

    fn stop(&mut self, stopped: Stopped) {
        self.stopped = Some(stopped);
    }
}

/// Refresh connected repositories, and stay open until the reading is done.
///
/// **The whole point is that the answer arrives here rather than somewhere else later.** Asking for a
/// listing is asynchronous by necessity — a repository can take half a minute, and a request held open for
/// that is a request that times out in between — so what the ask returns is a receipt. This dialog then
/// polls the connection until the run behind that receipt is over and says how it went, instead of closing
/// on "it was asked for" and leaving the reader to press Refresh at a tree and guess.
///
/// It covers one connection when it is opened from one, and every connection when it is opened from the
/// root of the mirrors — one repository or eight, the shape is the same and so is the waiting.
#[component]
pub fn RefreshGithubDialog(
    project: String,
    connections: Vec<String>,
    /// Which press this is. It moves whether or not the numbers below do — see
    /// `DialogState::RefreshGithub`.
    attempt: usize,
    /// What the router started, or `None` before the button has been pressed. Set is what wakes the
    /// watch — see [`GithubRefreshRun`].
    runs: Option<Vec<GithubRefreshRun>>,
    on_submit: EventHandler<()>,
    /// Called once, when everything being watched has landed. The mirror may hold different files than it
    /// did, so what opened this is holding a listing that is out of date.
    on_finished: EventHandler<()>,
) -> Element {
    let mut cs = use_signal(ComponentState::default);

    // The router re-opens this dialog carrying the runs it started, and that is what starts the watch. A
    // prop is not a signal, so `use_reactive!` is what makes a change to one run this at all — and it runs
    // this on a CHANGE, which is why `attempt` is in the dependencies beside the runs it is watching: two
    // presses can hand back identical numbers, and the second one still has to start a watch.
    let watch_project = project.clone();

    use_effect(use_reactive!(|attempt, runs| {
        let _ = attempt;

        if let Some(runs) = runs.clone() {
            let generation = cs.write().begin_watch();
            watch(cs, watch_project.clone(), runs, generation, on_finished);
        }
    }));

    let cs_ra = cs.read();

    let feedback = super::feedback();

    let watched = match get_connections(cs, &cs_ra, &project, &connections) {
        Ok(watched) => watched.to_vec(),
        Err(element) => {
            return super::dialog_template("Refresh from GitHub", element, rsx! {});
        }
    };

    let stopped = cs_ra.stopped.clone();

    drop(cs_ra);

    // Still asking: something was started and nothing has said it is over.
    let watching = runs.is_some() && stopped.is_none();

    let content = render_body(
        &connections,
        &watched,
        runs.as_deref(),
        stopped.as_ref(),
        &feedback.error,
    );

    let label = if feedback.saving {
        "Asking…"
    } else if watching {
        "Reading…"
    } else if stopped.is_some() {
        "Refresh again"
    } else {
        "Refresh"
    };

    let ok = rsx! {
        button {
            class: "btn btn-primary",
            disabled: feedback.saving || watching,
            onclick: move |_| {
                // How the last run ended is not the answer to the one being asked for now.
                let _ = cs.write().begin_watch();
                on_submit.call(());
            },
            "{label}"
        }
    };

    super::dialog_template("Refresh from GitHub", content, ok)
}

/// Ask, and keep asking until every run being watched has ended.
///
/// Each tick reads the whole connections list rather than one connection: there is one call that answers
/// it, it answers out of memory, and a refresh of eight repositories then costs the same as a refresh of
/// one.
fn watch(
    mut cs: Signal<ComponentState>,
    project: String,
    runs: Vec<GithubRefreshRun>,
    generation: usize,
    on_finished: EventHandler<()>,
) {
    spawn(async move {
        let mut waited = Duration::ZERO;

        // The rows to keep, in the order they were asked for — the runs are what this watch is about, so
        // a connection nobody asked about is not drawn even if the answer carries it.
        let watching: Vec<String> = runs.iter().map(|itm| itm.connection.clone()).collect();

        loop {
            let answer = crate::api::get_github_connections(&project).await;

            // A newer watch has taken over — this one is asking about numbers that have been superseded,
            // so it says nothing and stops. Read into a local: writing while a read guard is alive is a
            // deadlock, and the guard of an `if` condition is not obviously dead.
            let current = cs.read().is_current(generation);

            if !current {
                return;
            }

            match answer {
                Ok(response) => {
                    let watched = watched_connections(response.connections, &watching);
                    let landed = every_run_is_over(&watched, &runs);

                    cs.write().set_polled(watched, landed);

                    if landed {
                        // The mirror may hold different files than it did, so whatever opened this is
                        // holding a listing that is now out of date.
                        on_finished.call(());
                        return;
                    }
                }
                Err(err) => {
                    // About reading the state back, not about the repository — the listing itself is
                    // somewhere else and may well be finishing right now. Said as itself rather than
                    // dressed up as a failed refresh.
                    cs.write().stop(Stopped::Failed(err.message));
                    return;
                }
            }

            if waited >= GIVE_UP_AFTER {
                cs.write().stop(Stopped::GaveUp);
                return;
            }

            dioxus_utils::js::sleep(POLL_EVERY).await;
            waited += POLL_EVERY;
        }
    });
}

/// The connections this dialog is about, in the order they were asked for.
///
/// Ordered by the ask rather than by the answer so the rows do not move under a reader between two ticks
/// of the watch — the list they are watching is the list they opened.
fn watched_connections(
    connections: Vec<GithubConnectionResponse>,
    watching: &[String],
) -> Vec<GithubConnectionResponse> {
    watching
        .iter()
        .filter_map(|name| connections.iter().find(|itm| &itm.name == name).cloned())
        .collect()
}

/// Whether every run being watched has ended.
///
/// A connection that is no longer in the answer counts as over: it has been detached while this was open,
/// so there is nothing left to wait for and waiting anyway would hang on a row that no longer exists.
fn every_run_is_over(connections: &[GithubConnectionResponse], runs: &[GithubRefreshRun]) -> bool {
    runs.iter().all(|run| run_is_over(connections, run))
}

fn run_is_over(connections: &[GithubConnectionResponse], run: &GithubRefreshRun) -> bool {
    match connections.iter().find(|itm| itm.name == run.connection) {
        Some(connection) => connection.pull_no > run.pull_no,
        None => true,
    }
}

fn render_body(
    watching: &[String],
    connections: &[GithubConnectionResponse],
    runs: Option<&[GithubRefreshRun]>,
    stopped: Option<&Stopped>,
    error: &str,
) -> Element {
    rsx! {
        div { class: "confirm-body",
            {render_intro(watching, runs.is_some())}

            div { class: "github-refresh",
                for connection in connections.iter() {
                    RenderRow {
                        key: "{connection.name}",
                        connection: connection.clone(),
                        // The row's own run, when one was started for it.
                        run: runs.and_then(|runs| {
                            runs.iter().find(|itm| itm.connection == connection.name).cloned()
                        }),
                    }
                }
            }

            {render_outcome(connections, stopped)}

            if !error.is_empty() {
                div { class: "error-note", "{error}" }
            }
        }
    }
}

/// What pressing the button will do, said before it is pressed and not repeated afterwards.
///
/// **A refresh is worth a sentence because it is not the copy-everything-down it looks like.** What
/// arrives is merged only into a working copy with nothing to lose, so somebody pressing this to have
/// their edited file replaced by the repository's version is owed that before the press rather than after.
fn render_intro(watching: &[String], started: bool) -> Element {
    if started {
        return rsx! {
            p { class: "field-hint",
                "Cloning from GitHub. This stays open until it is done — the whole repository comes down again, which can be minutes on a large one. The copy that is there now stays readable for all of it and is replaced only when the new clone is complete."
            }
        };
    }

    let what = match watching {
        [one] => rsx! {
            "Clone "
            span { class: "mono", "{one}" }
            " from GitHub again, and put the new copy in place of the one on this disk."
        },
        many => rsx! {
            "Clone all {many.len()} connected repositories from GitHub again, and put the new copies in place of the ones on this disk."
        },
    };

    rsx! {
        p { {what} }
        p { class: "field-hint",
            "A refresh is not a fetch — the ten-minute timer already does that. Taking the repository down again is what makes a folder right when a fetch cannot: a force-pushed branch, a file that stopped being tracked, a copy left on the wrong branch. Nothing is deleted first: the new clone is made beside the old one and swapped in when it is complete, so the folder stays readable throughout and a refresh that fails leaves it exactly as it is. What the swap does discard is anything an agent left in the old copy through a git command — push that first if it matters."
        }
    }
}

/// The one line that says how it went, once there is something to say.
fn render_outcome(connections: &[GithubConnectionResponse], stopped: Option<&Stopped>) -> Element {
    match stopped {
        None => rsx! {},
        Some(Stopped::Landed) => {
            // Not "it worked": a listing that ends in `failed` or `needs-key` has ended just as
            // definitely, and each row is already saying which. This says whether the reader is done.
            let unread: Vec<&str> = connections
                .iter()
                .filter(|itm| itm.state != GithubMirrorState::READY)
                .map(|itm| itm.name.as_str())
                .collect();

            if unread.is_empty() {
                rsx! {
                    // It can be said plainly now, and only because of what a refresh became: the folder
                    // was deleted and cloned, so there is no working copy that quietly declined to move
                    // and no sentence to hedge with.
                    div { class: "refresh-done", "Cloned. These folders are now exactly what GitHub holds." }
                }
            } else {
                let unread = unread.join(", ");

                rsx! {
                    div { class: "refresh-done failed",
                        "Finished — {unread} could not be read. The reason is beside it."
                    }
                }
            }
        }
        Some(Stopped::GaveUp) => rsx! {
            div { class: "refresh-done failed",
                "This is taking longer than a clone of a large repository normally does. It may still be running — close this and open the folder again in a few minutes."
            }
        },
        Some(Stopped::Failed(message)) => rsx! {
            div { class: "error-note",
                "Could not read the connections back, so there is no way to tell from here whether the listing finished: {message}"
            }
        },
    }
}

/// One connection: what it is, what it holds, and — while a run of its own is in flight — that it is being
/// read.
#[component]
fn RenderRow(connection: GithubConnectionResponse, run: Option<GithubRefreshRun>) -> Element {
    // In flight when a run was started for this row and the count has not moved past where it started.
    let reading = run
        .map(|run| connection.pull_no <= run.pull_no)
        .unwrap_or(false);

    let branch = if connection.branch.is_empty() {
        "default branch".to_string()
    } else {
        connection.branch.clone()
    };

    rsx! {
        div { class: "refresh-row",
            div { class: "refresh-name",
                div { class: "mono", "{connection.name}" }
                div { class: "field-hint",
                    "{connection.repo} · {branch}"
                    if !connection.path.is_empty() {
                        " · {connection.path}/"
                    }
                }
            }
            div { class: "refresh-state",
                // The first line is the only one that changes while the listing runs, and the ones under
                // it stay where they are: a row that loses a line and gets it back reads as the screen
                // being rebuilt rather than as one word being replaced.
                if reading {
                    div { "reading…" }
                } else {
                    div { "{github_state_text(&connection)}" }
                }
                div { class: "field-hint",
                    if connection.commit.is_empty() {
                        "{read_ago(connection.pulled_unix_seconds)}"
                    } else {
                        "{connection.commit} · {read_ago(connection.pulled_unix_seconds)}"
                    }
                }
                if !connection.error.is_empty() && !reading {
                    div { class: "field-hint", "{connection.error}" }
                }
            }
        }
    }
}

/// What a connection's state says, in a sentence rather than a keyword.
///
/// One copy, shared with the connections dialog: two screens describing the same five states in two
/// vocabularies is how a reader ends up believing they are looking at two different things.
///
/// The vocabulary travels as an open string — an unknown one is drawn as itself rather than swallowed,
/// which is the same leniency every other open vocabulary in this client gets.
pub fn github_state_text(connection: &GithubConnectionResponse) -> String {
    let files = if connection.skipped_amount > 0 {
        format!(
            "{} files ({} not mirrored)",
            connection.files_amount, connection.skipped_amount
        )
    } else {
        format!("{} files", connection.files_amount)
    };

    match connection.state.as_str() {
        GithubMirrorState::READY => files,
        GithubMirrorState::PULLING => "reading…".to_string(),
        GithubMirrorState::PENDING => "not read yet".to_string(),
        GithubMirrorState::NEEDS_KEY => {
            if connection.has_key {
                "the key is not enough".to_string()
            } else {
                "needs a key".to_string()
            }
        }
        // A failure keeps whatever the last good read left, so what is still readable is worth saying
        // beside the fact that the last attempt did not work.
        GithubMirrorState::FAILED => format!("failed · {files} still readable"),
        other => other.to_string(),
    }
}

/// How long ago the listing this row is showing was made.
///
/// An age rather than a timestamp: what a reader is checking is whether they are looking at something
/// stale, and "read 9 minutes ago" answers that where "12:31" is arithmetic they have to do.
fn read_ago(pulled_unix_seconds: i64) -> String {
    if pulled_unix_seconds == 0 {
        return "never read".to_string();
    }

    let now = js_sys::Date::now() as i64 / 1_000;
    let seconds = (now - pulled_unix_seconds).max(0);

    if seconds <= 45 {
        return "read just now".to_string();
    }

    let minutes = (seconds + 30) / 60;

    match minutes {
        0..=1 => "read a minute ago".to_string(),
        2..=90 => format!("read {minutes} minutes ago"),
        _ => {
            let hours = (seconds + 1_800) / 3_600;

            match hours {
                0..=1 => "read an hour ago".to_string(),
                _ => format!("read {hours} hours ago"),
            }
        }
    }
}

/// The connections, loaded once when the dialog opens.
///
/// Its own request rather than a list handed in by whatever opened it: this dialog is about to change
/// exactly these rows, and a picture taken before the button was pressed is the one thing it must not be
/// showing afterwards. Every tick of the watch writes the answer back into the same slot.
fn get_connections(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
    project: &str,
    watching: &[String],
) -> Result<Vec<GithubConnectionResponse>, Element> {
    match cs_ra.connections.as_ref() {
        RenderState::None => {
            let project = project.to_string();

            spawn(async move {
                cs.write().connections.set_loading();

                match crate::api::get_github_connections(&project).await {
                    Ok(response) => cs.write().connections.set_loaded(response.connections),
                    Err(err) => cs.write().connections.set_error(err.message),
                }
            });

            Err(rsx! {
                div { class: "loading-note", "Loading…" }
            })
        }
        RenderState::Loading => Err(rsx! {
            div { class: "loading-note", "Loading…" }
        }),
        RenderState::Loaded(connections) => Ok(watched_connections(connections.clone(), watching)),
        RenderState::Error(err) => Err(rsx! {
            div { class: "error-note", "{err}" }
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(name: &str, pull_no: i64) -> GithubConnectionResponse {
        GithubConnectionResponse {
            name: name.to_string(),
            repo: "owner/repo".to_string(),
            branch: String::new(),
            path: String::new(),
            has_key: false,
            state: GithubMirrorState::READY.to_string(),
            error: String::new(),
            commit: "a1b2c3d".to_string(),
            pulled_unix_seconds: 0,
            files_amount: 3,
            skipped_amount: 0,
            pull_no,
        }
    }

    fn run(name: &str, pull_no: i64) -> GithubRefreshRun {
        GithubRefreshRun {
            connection: name.to_string(),
            pull_no,
        }
    }

    /// The whole contract of the watch: the run is over when the count has moved PAST where it started,
    /// and not when it merely equals it.
    #[test]
    fn a_run_is_over_when_the_count_has_passed_where_it_started() {
        assert!(!run_is_over(&[connection("specs", 4)], &run("specs", 4)));
        assert!(run_is_over(&[connection("specs", 5)], &run("specs", 4)));
    }

    /// A refresh of eight repositories is not over because one of them came back.
    #[test]
    fn every_run_has_to_be_over_not_just_one() {
        let runs = [run("specs", 1), run("api", 7)];

        assert!(!every_run_is_over(
            &[connection("specs", 2), connection("api", 7)],
            &runs
        ));

        assert!(every_run_is_over(
            &[connection("specs", 2), connection("api", 8)],
            &runs
        ));
    }

    /// Detached while the dialog was open. There is nothing left to wait for, and waiting anyway would
    /// hang on a row that is not on the screen either.
    #[test]
    fn a_connection_that_is_gone_is_not_waited_for() {
        assert!(every_run_is_over(&[], &[run("specs", 1)]));
    }

    /// The rows follow the list that was asked for, so they cannot reorder under a reader between two
    /// ticks — and a name that no longer exists simply is not drawn.
    #[test]
    fn the_rows_are_the_ones_that_were_asked_for_in_that_order() {
        let answer = vec![connection("api", 0), connection("specs", 0)];

        let watched = watched_connections(answer, &["specs".to_string(), "gone".to_string()]);

        assert_eq!(watched.len(), 1);
        assert_eq!(watched[0].name, "specs");
    }
}
