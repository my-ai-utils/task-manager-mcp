use dioxus_utils::DataState;
use task_manager_shared::auth::MeResponse;
use task_manager_shared::ws::BoardSnapshot;

/// Not `Clone` and not `PartialEq`, because `signed_in` is a `DataState` and that is neither. Nothing
/// ever cloned or compared the whole struct — the fields that are read are read one at a time — so the
/// derives were only ever holding the state machine below in a hand-rolled shape.
pub struct AppState {
    /// Who is signed in, as `/me` answered. The three render states are exactly the three things the
    /// shell can show — a spinner, the login screen, the product — which is why this is a `DataState`
    /// rather than a bespoke enum: "not asked yet" is `None`, and "asked, nobody is" is `Loaded(None)`.
    ///
    /// `Option` inside, because a signed-out browser is an ANSWER and not a failure: the endpoint says so
    /// deliberately, and collapsing it into `Error` would report a working sign-out as a fault.
    pub signed_in: DataState<Option<MeResponse>>,
    /// True once the WebSocket task has been spawned, so a re-render does not open a second one.
    pub ws_started: bool,
    /// Bumped by the WebSocket on every push about the open board. Views watch it, which is what makes two
    /// pushes carrying identical tasks still count as two pushes.
    pub board_revision: u64,
    /// The board the last push carried, when it carried one. `None` means the push was a bare signal and the
    /// view has to re-read — see [`AppState::board_invalidated`].
    ///
    /// The socket lands it here rather than writing into Home's own state because the socket is older than
    /// any screen and outlives all of them: it has no way to reach a component's signal, and Home watching
    /// this is the same shape as Home watching the revision counter it replaces.
    pub board_push: Option<BoardSnapshot>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            signed_in: DataState::new(),
            ws_started: false,
            board_revision: 0,
            board_push: None,
        }
    }
}

impl AppState {
    /// Whoever is signed in — `None` while `/me` is still out, and `None` when it came back with nobody.
    /// The two are told apart by the shell, which is the only place that has anything different to do
    /// about them; everywhere else the question is only "have I got a user".
    pub fn me(&self) -> Option<&MeResponse> {
        self.signed_in.try_unwrap_as_loaded()?.as_ref()
    }

    pub fn is_admin(&self) -> bool {
        self.me().map(|me| me.is_admin).unwrap_or(false)
    }

    /// A push that carried the board. The view replaces what it shows with this — no request, no spinner.
    ///
    /// One method for the two fields because they are one fact: a snapshot without the bump would not wake a
    /// view that is already showing an equal board, and a bump without the snapshot means the opposite thing.
    pub fn board_pushed(&mut self, snapshot: BoardSnapshot) {
        self.board_revision += 1;
        self.board_push = Some(snapshot);
    }

    /// A push that only said "it changed". Clearing the snapshot is the point: leaving the previous one in
    /// place would have the view re-apply a board that is now known to be out of date.
    pub fn board_invalidated(&mut self) {
        self.board_revision += 1;
        self.board_push = None;
    }
}
