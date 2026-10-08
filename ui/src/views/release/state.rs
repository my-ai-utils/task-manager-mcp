use dioxus_utils::DataState;
use task_manager_shared::releases::ReleaseResponse;
use task_manager_shared::ws::BoardSnapshot;

/// One release, on a page of its own.
///
/// The address is held here beside what was read from it, and that pairing is the point: the page is
/// reached by a link, and a link to another release opened from this one changes the address without
/// building a new page. What is on screen has to be known to belong to the address in the bar.
pub struct ComponentState {
    /// The two halves of the address, as they were typed: the board's prefix, and the release's id or
    /// its bare number.
    pub project: String,
    pub release_ref: String,
    pub release: DataState<ReleaseResponse>,
    /// The last push this page has acted on, by the app's own count of them — see
    /// [`Self::board_changed`].
    pub seen_revision: u64,
}

impl ComponentState {
    pub fn new(project: String, release_ref: String) -> Self {
        Self {
            project,
            release_ref,
            release: DataState::new(),
            seen_revision: 0,
        }
    }

    /// Whether this is the address the page is already on. Asked BEFORE [`Self::open`], through a peek:
    /// the effect that follows the address runs on the first render too, with the very address the state
    /// was created from, and a write that changed nothing would still be a write — a second render of a
    /// page that has not finished its first.
    pub fn is_at(&self, project: &str, release_ref: &str) -> bool {
        self.project == project && self.release_ref == release_ref
    }

    /// The address changed under the page. The same address is not a change, and nothing is thrown away
    /// for it.
    pub fn open(&mut self, project: &str, release_ref: &str) {
        if self.is_at(project, release_ref) {
            return;
        }

        self.project = project.to_string();
        self.release_ref = release_ref.to_string();
        // Reset rather than cleared: the next render sees `None` and reads, the path a first visit takes.
        self.release.reset();
    }

    /// Something arrived over the socket since this page last looked: `revision` is the app's count of
    /// pushes, and `push` is the board the last one carried — `None` for one that only said "it changed".
    ///
    /// **Counted, and that is the whole of the rule.** "No board was carried" is also what the app holds
    /// before anything has been pushed at all, and the app's state changes for other reasons — the socket
    /// being started is one, and this page starts it the moment its release has arrived. Taken on its
    /// own, a missing board would read as "it changed" then, and the page would throw away the release it
    /// had just read and ask for it again. A push is a revision that moved; nothing else is.
    pub fn board_changed(&mut self, revision: u64, push: Option<&BoardSnapshot>) {
        if self.seen_revision == revision {
            return;
        }

        self.seen_revision = revision;

        match push {
            Some(snapshot) => self.board_pushed(snapshot),
            None => self.board_invalidated(),
        }
    }

    /// A push that carried a board. When it is this release's board, what it says about the release
    /// replaces what is shown, with no request.
    ///
    /// **A board that no longer lists the release is a release that has just been deleted** — a push
    /// carries the live ones — and that is not something the snapshot can show: so the page forgets what
    /// it has and reads again, and the server's answer is the release marked as deleted.
    ///
    /// Nothing happens before the first read has landed: until then there is no id to look for, since the
    /// address may have named the release by its number.
    fn board_pushed(&mut self, snapshot: &BoardSnapshot) {
        let Some(shown) = self.release.try_unwrap_as_loaded() else {
            return;
        };

        if snapshot.project != shown.project {
            return;
        }

        let pushed = snapshot.releases.iter().find(|itm| itm.id == shown.id);
        let changed = pushed.is_some_and(|pushed| pushed != shown);
        let shown_as_deleted = shown.deleted_unix_seconds.is_some();

        match pushed {
            Some(pushed) if changed => self.release.set_loaded(pushed.clone()),
            Some(_) => {}
            // Already shown as deleted, and still not on the board: there is nothing new to read.
            None if shown_as_deleted => {}
            None => self.release.reset(),
        }
    }

    /// A push that only said "it changed": forget what is shown, and the next render reads it again.
    fn board_invalidated(&mut self) {
        if self.release.has_value() {
            self.release.reset();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(id: &str, title: &str) -> ReleaseResponse {
        ReleaseResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            title: title.to_string(),
            description: String::new(),
            release_notes: String::new(),
            date_unix_seconds: 0,
            services: Vec::new(),
            goals: Vec::new(),
            envs: Vec::new(),
            done_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    fn snapshot(project: &str, releases: Vec<ReleaseResponse>) -> BoardSnapshot {
        BoardSnapshot {
            project: project.to_string(),
            tasks: Vec::new(),
            goals: Vec::new(),
            releases,
        }
    }

    fn showing(release: ReleaseResponse) -> ComponentState {
        let mut state = ComponentState::new("RMS".to_string(), "12".to_string());
        state.release.set_loaded(release);
        state
    }

    fn shown(state: &ComponentState) -> Option<&ReleaseResponse> {
        state.release.try_unwrap_as_loaded()
    }

    /// The address a release is linked by and the address the router reads are written in two places —
    /// the function that builds a link, and the route. This is what holds them to one shape: a link that
    /// the app itself hands out has to be one the app itself opens.
    #[test]
    fn the_link_to_a_release_is_the_address_of_its_page() {
        let release = release("RMS-R12", "Releases");

        let route = crate::AppRoute::ReleasePage {
            project: release.project.clone(),
            release: release.id.clone(),
        };

        assert_eq!(
            route.to_string(),
            task_manager_shared::releases::release_page_path(&release)
        );
    }

    /// A link to another release, followed from this page, changes the address without building a new
    /// page — so what is shown has to be dropped, or the bar would say one release and the page another.
    #[test]
    fn another_address_drops_what_was_read_and_the_same_one_does_not() {
        let mut state = showing(release("RMS-R12", "Releases"));

        state.open("RMS", "12");
        assert!(shown(&state).is_some(), "the same address is not a change");

        state.open("RMS", "RMS-R13");
        assert!(shown(&state).is_none());
        assert_eq!(state.release_ref, "RMS-R13");

        let mut state = showing(release("RMS-R12", "Releases"));
        state.open("TM", "12");
        assert!(shown(&state).is_none(), "the same number on another board is another release");
    }

    /// The page is live the way every screen is: a change somebody makes to the release — a label added
    /// when it reaches production — arrives without a reload.
    #[test]
    fn a_push_about_this_release_replaces_what_is_shown() {
        let mut state = showing(release("RMS-R12", "Releases"));

        let mut on_prod = release("RMS-R12", "Releases");
        on_prod.envs = vec!["Prod".to_string()];

        state.board_changed(
            1,
            Some(&snapshot(
                "RMS",
                vec![release("RMS-R13", "Something else"), on_prod],
            )),
        );

        assert_eq!(shown(&state).unwrap().envs, vec!["Prod"]);
    }

    /// The page starts the socket once its release has arrived, and starting it changes the app's state
    /// with no push behind the change. That must not read as "the board changed": the page would drop
    /// the release it had just read and ask for it a second time, on every single visit.
    #[test]
    fn nothing_pushed_is_not_a_push() {
        let mut state = showing(release("RMS-R12", "Releases"));

        // The app as it is before any push: revision 0, and no board.
        state.board_changed(0, None);
        assert!(shown(&state).is_some(), "there has been no push to act on");

        // A real one that carried nothing: now the page reads again — once.
        state.board_changed(1, None);
        assert!(shown(&state).is_none());

        state.release.set_loaded(release("RMS-R12", "Releases"));
        state.board_changed(1, None);
        assert!(shown(&state).is_some(), "the same push is not acted on twice");
    }

    /// Another board's push is not about this release, however alike the ids — and a push that arrives
    /// before the first read has nothing to be compared with.
    #[test]
    fn a_push_about_another_board_or_before_the_first_read_changes_nothing() {
        let mut state = showing(release("RMS-R12", "Releases"));

        state.board_changed(
            1,
            Some(&snapshot("TM", vec![release("RMS-R12", "An impostor")])),
        );
        assert_eq!(shown(&state).unwrap().title, "Releases");

        let mut unread = ComponentState::new("RMS".to_string(), "12".to_string());
        unread.board_changed(
            1,
            Some(&snapshot("RMS", vec![release("RMS-R12", "Releases")])),
        );
        assert!(shown(&unread).is_none(), "the read decides what the address names, not a push");
    }

    /// A push carries the live releases, so one that has left the board has been deleted — which the page
    /// must say rather than go on showing the release as it was. It asks the server, once.
    #[test]
    fn a_release_that_left_the_board_is_read_again_and_only_once() {
        let mut state = showing(release("RMS-R12", "Releases"));

        state.board_changed(1, Some(&snapshot("RMS", Vec::new())));
        assert!(shown(&state).is_none(), "forgotten, so the next render reads it");

        // The read came back marked as deleted. The next push still does not list it, and must not send
        // the page round again.
        let mut deleted = release("RMS-R12", "Releases");
        deleted.deleted_unix_seconds = Some(1_791_331_200);
        state.release.set_loaded(deleted);

        state.board_changed(2, Some(&snapshot("RMS", Vec::new())));
        assert!(shown(&state).is_some());

        // Brought back: it is on the board again, and the page shows it as live.
        state.board_changed(
            3,
            Some(&snapshot("RMS", vec![release("RMS-R12", "Releases")])),
        );
        assert!(shown(&state).unwrap().deleted_unix_seconds.is_none());
    }
}
