use dioxus_utils::DataState;
use serde::{Deserialize, Serialize};
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::releases::{ReleaseResponse, is_done, is_on_env, same_env};

/// The two questions the environment filter can ask, as the prefix of its value: `on:Prod` is the releases
/// that are out on Prod, `not:Prod` the ones that are not there yet. Empty is "any".
///
/// A string rather than an enum because it is also the value of the `<option>` that sets it, and one
/// vocabulary for the control and the state is what keeps the two from drifting. The label follows the
/// colon whole, so an environment may be called anything a label may.
pub const ON_ENV: &str = "on:";
pub const NOT_ON_ENV: &str = "not:";

/// The two values the state filter can take besides "any", which is the empty string: the releases whose
/// rollout is still going, and the ones that have been closed.
///
/// Strings rather than an enum for the reason the environment filter's are: they are also the values of
/// the `<option>`s that set them.
pub const IN_PROGRESS: &str = "open";
pub const DONE: &str = "done";

/// How the Releases screen was left — what this browser keeps of it, in `localStorage`.
///
/// **The screen is not drawn from this; its state is CREATED from it.** [`ComponentState::new`] reads the
/// record once, so the first thing drawn is already the screen the reader left, and from then on the
/// state is the only thing anything looks at. The other direction is [`ComponentState::persist`], called
/// by every method that changes one of these fields — the state and storage move together, and nothing
/// else writes.
///
/// `localStorage` rather than `sessionStorage`, because these are preferences and not a place in a
/// visit: "show me what is still in progress" is how somebody wants releases shown tomorrow too, in
/// whatever tab they open.
///
/// Every field has a default, so a record written by an older build still loads — and the default for
/// `done_filter` is not empty: a browser that has never been here opens on what is still in progress.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasesRecord {
    /// The board the record was written on. It qualifies `service_filter` and nothing else: a service is
    /// a service of ONE board, so the filter is honoured only when the screen opens on this board again.
    /// Which board that is, is not decided here — it is the one every screen remembers together.
    #[serde(default)]
    pub board: String,
    #[serde(default)]
    pub service_filter: String,
    #[serde(default)]
    pub env_filter: String,
    #[serde(default = "in_progress")]
    pub done_filter: String,
}

fn in_progress() -> String {
    IN_PROGRESS.to_string()
}

impl Default for ReleasesRecord {
    /// A first visit: any service, any environment, and what is still in progress.
    fn default() -> Self {
        Self {
            board: String::new(),
            service_filter: String::new(),
            env_filter: String::new(),
            done_filter: in_progress(),
        }
    }
}

pub struct ComponentState {
    pub projects: DataState<Vec<ProjectResponse>>,
    /// Which board is on screen, by PREFIX — the same vocabulary Home and Goals hold and the api speaks.
    /// Empty until the projects have arrived: a remembered board is a wish until the list says the
    /// reader may still see it.
    pub selected: String,
    pub releases: DataState<Vec<ReleaseResponse>>,
    /// Which releases are open, by id. Kept across a repaint, so a push does not fold up what somebody
    /// was reading — and ids rather than positions, because a release recorded meanwhile moves every row.
    pub expanded: Vec<String>,
    /// Which microservice is being looked at, by its id — empty for all of them.
    pub service_filter: String,
    /// Which releases are on screen by where they are out: [`ON_ENV`] or [`NOT_ON_ENV`] followed by an
    /// environment's label, or empty for all of them. Deliberately NOT reset by `select`, unlike the
    /// service above: it is how this reader wants releases shown — "what is live" — and not something
    /// about one board. A board that has no such environment still shows the choice in the control, so
    /// what emptied the list can be seen — see [`envs_to_offer`].
    pub env_filter: String,
    /// Which releases are on screen by whether they have been closed: [`IN_PROGRESS`], [`DONE`], or
    /// empty for both. Not reset by `select` either, and for the same reason: "what still has somewhere
    /// to go" is asked of one board after another.
    pub done_filter: String,
    /// The board the stored filters were written on — see [`ReleasesRecord::board`]. Read when the
    /// projects arrive, to decide whether the remembered service still means anything.
    filters_board: String,
    /// The board this browser was last on, as storage holds it. Shared by every screen — it is what
    /// keeps the tabs on one board — and kept here so that choosing the board already remembered is not
    /// a write.
    remembered_board: Option<String>,
    /// What storage holds of this screen right now. It is what lets [`Self::persist`] be called by every
    /// method that MAY have changed the record and still write only when one did.
    stored: Option<ReleasesRecord>,
}

impl ComponentState {
    /// The state as this browser left it.
    ///
    /// **The one read of storage this screen makes.** Not in the render body and not in an effect: a
    /// state created empty and filled in afterwards draws a wrong first frame and then corrects it, and
    /// a screen drawn from storage directly has two sources that can disagree.
    pub fn new() -> Self {
        let stored = crate::web::storage::releases::get();
        let record = stored.clone().unwrap_or_default();

        Self {
            projects: DataState::new(),
            selected: String::new(),
            releases: DataState::new(),
            expanded: Vec::new(),
            service_filter: record.service_filter,
            env_filter: record.env_filter,
            done_filter: record.done_filter,
            filters_board: record.board,
            remembered_board: crate::web::storage::get_last_project(),
            stored,
        }
    }

    /// The projects have arrived: now the board can be chosen, and the stored filters checked against
    /// it.
    pub fn projects_loaded(&mut self, projects: Vec<ProjectResponse>) {
        self.selected = board_to_open(&projects, self.remembered_board.as_deref());

        // A service is a service of one board. The remembered one is kept only when the screen is back
        // on the board it was chosen on — the board may have been changed since from another screen,
        // or have gone from this reader's list — and anywhere else it would empty the list for a
        // reason nobody could see.
        if self.filters_board != self.selected {
            self.service_filter.clear();
        }

        self.projects.set_loaded(projects);

        self.remember_board();
        self.persist();
    }

    pub fn select(&mut self, prefix: String) {
        if self.selected == prefix {
            return;
        }

        self.selected = prefix;
        // Reset rather than clear: the next render sees `None` and loads, the path a first visit takes.
        self.releases.reset();
        self.expanded.clear();
        // Unlike the Goals screen's filters this one IS about one board: it names a service of the project
        // just left, and carried over it would empty the next one for a reason nobody could see.
        self.service_filter.clear();

        self.remember_board();
        self.persist();
    }

    pub fn toggle(&mut self, id: &str) {
        if let Some(at) = self.expanded.iter().position(|itm| itm == id) {
            self.expanded.remove(at);
        } else {
            self.expanded.push(id.to_string());
        }
    }

    pub fn set_service_filter(&mut self, microservice_id: String) {
        self.service_filter = microservice_id;
        self.persist();
    }

    pub fn set_env_filter(&mut self, value: String) {
        self.env_filter = value;
        self.persist();
    }

    pub fn set_done_filter(&mut self, value: String) {
        self.done_filter = value;
        self.persist();
    }

    /// The ONE place this state becomes a record.
    fn to_record(&self) -> ReleasesRecord {
        ReleasesRecord {
            board: self.selected.clone(),
            service_filter: self.service_filter.clone(),
            env_filter: self.env_filter.clone(),
            done_filter: self.done_filter.clone(),
        }
    }

    /// The only writer of the record, and it compares before it writes — so it is safe at the end of
    /// every method that may have changed one of the stored fields, and choosing what is already chosen
    /// is not a write.
    fn persist(&mut self) {
        let record = self.to_record();

        if self.stored.as_ref() == Some(&record) {
            return;
        }

        crate::web::storage::releases::set(&record);
        self.stored = Some(record);
    }

    /// The board on screen becomes the board every screen opens on. Written here — by the methods that
    /// choose a board — rather than by an effect watching the state, and only when it is not the one
    /// storage already has.
    fn remember_board(&mut self) {
        if self.selected.is_empty() || self.remembered_board.as_deref() == Some(self.selected.as_str())
        {
            return;
        }

        crate::web::storage::save_last_project(&self.selected);
        self.remembered_board = Some(self.selected.clone());
    }

    /// A push that carried the board: what it says replaces what is shown, with no request.
    pub fn board_pushed(&mut self, releases: Vec<ReleaseResponse>) {
        self.releases.set_loaded(releases);
    }

    /// A push that only said "it changed": forget the list, and the next render reads it again.
    pub fn board_invalidated(&mut self) {
        self.releases.reset();
    }
}

/// Which board the screen opens on: the one this browser was last on when the reader can still see it,
/// else the first live one, else whatever there is. The choice Home and Goals make, for the same reasons.
///
/// The remembered prefix is matched ignoring case and answered in the project's OWN spelling, so what
/// the state holds is always exactly what the picker's options carry.
pub fn board_to_open(projects: &[ProjectResponse], remembered: Option<&str>) -> String {
    remembered
        .and_then(|prefix| {
            projects
                .iter()
                .find(|itm| itm.prefix.eq_ignore_ascii_case(prefix))
        })
        .or_else(|| projects.iter().find(|itm| !itm.archived))
        .or_else(|| projects.first())
        .map(|itm| itm.prefix.clone())
        .unwrap_or_default()
}

/// The services the filter offers: the ones this board's releases name, and the one being filtered by
/// even when none of them does.
///
/// The second half matters now that the choice outlives a visit: the service somebody filtered by last
/// week may be in no release today. Left out of the control, that would be an empty list under a box
/// saying "Any service".
pub fn services_to_offer(releases: &[ReleaseResponse], filter: &str) -> Vec<String> {
    let mut result = microservices_of(releases);

    if !filter.is_empty() && !result.iter().any(|itm| itm == filter) {
        result.push(filter.to_string());
        result.sort();
    }

    result
}

/// Every microservice the releases name, once each, sorted — what the filter offers.
///
/// Read off the releases rather than kept anywhere, the way a board's labels are read off its tasks: a
/// service is on this list for exactly as long as some release names it.
pub fn microservices_of(releases: &[ReleaseResponse]) -> Vec<String> {
    let mut result: Vec<String> = releases
        .iter()
        .flat_map(|release| release.services.iter())
        .map(|service| service.microservice_id.clone())
        .collect();

    result.sort();
    result.dedup();
    result
}

/// Whether a release is on screen under the chosen filter. An empty filter shows everything.
pub fn includes_service(release: &ReleaseResponse, microservice_id: &str) -> bool {
    microservice_id.is_empty()
        || release
            .services
            .iter()
            .any(|service| service.microservice_id == microservice_id)
}

/// Whether a release is on screen under the state filter. Anything that is not one of the two values
/// shows everything — an empty box and a value this build does not know are the same screen.
pub fn matches_done_filter(release: &ReleaseResponse, filter: &str) -> bool {
    match filter {
        IN_PROGRESS => !is_done(release),
        DONE => is_done(release),
        _ => true,
    }
}

/// Every environment the releases name, once each however it is cased, sorted ignoring case.
///
/// Read off the releases rather than kept anywhere, like the services above: an environment is on this
/// list for exactly as long as some release is on it.
pub fn envs_of(releases: &[ReleaseResponse]) -> Vec<String> {
    let mut result: Vec<String> = Vec::new();

    for env in releases.iter().flat_map(|release| release.envs.iter()) {
        if !result.iter().any(|itm| same_env(itm, env)) {
            result.push(env.clone());
        }
    }

    result.sort_by_key(|itm| itm.to_lowercase());
    result
}

/// The environment a filter asks about, whichever way it asks. `None` for "any" — and for a value this
/// build does not know, which is the same screen.
pub fn env_of_filter(filter: &str) -> Option<&str> {
    filter
        .strip_prefix(ON_ENV)
        .or_else(|| filter.strip_prefix(NOT_ON_ENV))
        .map(str::trim)
        .filter(|itm| !itm.is_empty())
}

/// Whether a filter asks this `question` — [`ON_ENV`] or [`NOT_ON_ENV`] — about this environment.
///
/// What marks an option as the chosen one, and compared the way a label is, not letter for letter: the
/// filter is carried from board to board, and the next board may spell the same environment `prod` where
/// the last one had `Prod`. Matched exactly, the control would show "Any environment" over a list that
/// is in fact filtered.
pub fn filter_asks(filter: &str, question: &str, env: &str) -> bool {
    filter.starts_with(question) && env_of_filter(filter).is_some_and(|itm| same_env(itm, env))
}

/// The environments the filter offers: the ones this board's releases name, and the one being filtered
/// by even when none of them does.
///
/// The second half is what keeps the filter honest across boards. It is not reset when the board changes
/// — "what is on Prod" is asked of one project after another — so it can name an environment the board
/// now on screen has never used. Left out of the control, that would be an empty list with every filter
/// apparently off.
pub fn envs_to_offer(releases: &[ReleaseResponse], filter: &str) -> Vec<String> {
    let mut result = envs_of(releases);

    if let Some(chosen) = env_of_filter(filter) {
        if !result.iter().any(|itm| same_env(itm, chosen)) {
            result.push(chosen.to_string());
            result.sort_by_key(|itm| itm.to_lowercase());
        }
    }

    result
}

/// Whether a release is on screen under the environment filter. Anything that is not one of the two
/// questions shows everything — an empty box and a value this build does not know are the same screen.
pub fn matches_env_filter(release: &ReleaseResponse, filter: &str) -> bool {
    let Some(env) = env_of_filter(filter) else {
        return true;
    };

    if filter.starts_with(NOT_ON_ENV) {
        !is_on_env(release, env)
    } else {
        is_on_env(release, env)
    }
}

#[cfg(test)]
mod tests {
    use task_manager_shared::releases::ServiceReleaseResponse;

    use super::*;

    fn release(id: &str, services: &[&str]) -> ReleaseResponse {
        ReleaseResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            title: id.to_string(),
            description: String::new(),
            release_notes: String::new(),
            date_unix_seconds: 0,
            services: services
                .iter()
                .map(|microservice_id| ServiceReleaseResponse {
                    microservice_id: microservice_id.to_string(),
                    version: "1.0.0".to_string(),
                    git_hash: "099602e".to_string(),
                    release_link: String::new(),
                    datetime_unix_seconds: 0,
                    settings_update_note: String::new(),
                    description: String::new(),
                })
                .collect(),
            goals: Vec::new(),
            envs: Vec::new(),
            done_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    fn on(id: &str, envs: &[&str]) -> ReleaseResponse {
        let mut release = release(id, &["rest-api"]);
        release.envs = envs.iter().map(|itm| itm.to_string()).collect();
        release
    }

    fn project(prefix: &str, archived: bool) -> ProjectResponse {
        ProjectResponse {
            name: prefix.to_string(),
            description: String::new(),
            prefix: prefix.to_string(),
            prefix_history: Vec::new(),
            columns: Vec::new(),
            column_template_id: None,
            column_template_name: None,
            kinds: Vec::new(),
            kind_template_id: None,
            kind_template_name: None,
            members: Vec::new(),
            tasks_amount: 0,
            archive_days: None,
            archived,
        }
    }

    /// The screen as a browser opens it: created from whatever storage holds, then handed the boards.
    fn opened(boards: &[&str]) -> ComponentState {
        let mut state = ComponentState::new();
        state.projects_loaded(boards.iter().map(|prefix| project(prefix, false)).collect());
        state
    }

    fn stored() -> ReleasesRecord {
        crate::web::storage::releases::get().expect("the screen should have written its record")
    }

    /// A browser that has never been here opens on what still has somewhere to go — the list this
    /// screen is most often opened for — and not on the whole history.
    #[test]
    fn a_first_visit_shows_what_is_still_in_progress() {
        let state = opened(&["RMS", "TM"]);

        assert_eq!(state.selected, "RMS");
        assert_eq!(state.done_filter, IN_PROGRESS);
        assert!(state.env_filter.is_empty());
        assert!(state.service_filter.is_empty());
    }

    /// The whole point: what somebody chose is what they find next time — the board, and all three
    /// filters, including "Any state", which is a choice and must not be mistaken for no choice.
    #[test]
    fn the_screen_comes_back_the_way_it_was_left() {
        let mut state = opened(&["RMS", "TM"]);

        state.select("TM".to_string());
        state.set_service_filter("margin-engine".to_string());
        state.set_env_filter("on:Prod".to_string());
        state.set_done_filter(DONE.to_string());

        let back = opened(&["RMS", "TM"]);

        assert_eq!(back.selected, "TM");
        assert_eq!(back.service_filter, "margin-engine");
        assert_eq!(back.env_filter, "on:Prod");
        assert_eq!(back.done_filter, DONE);

        let mut back = back;
        back.set_done_filter(String::new());

        assert_eq!(
            opened(&["RMS", "TM"]).done_filter,
            "",
            "\"Any state\" was chosen, so it is what comes back — not the default"
        );
    }

    /// Every change of a stored field is in storage by the time the method returns: the state and the
    /// record move together, with no effect in between that could lag or be forgotten.
    #[test]
    fn a_change_is_written_by_the_method_that_makes_it() {
        let mut state = opened(&["RMS", "TM"]);

        state.set_env_filter("not:Prod".to_string());
        assert_eq!(stored().env_filter, "not:Prod");

        state.set_done_filter(DONE.to_string());
        assert_eq!(stored().done_filter, DONE);

        state.set_service_filter("rest-api".to_string());
        assert_eq!(stored().service_filter, "rest-api");
        assert_eq!(stored().board, "RMS", "and the record says which board that service is of");

        state.select("TM".to_string());
        assert_eq!(stored().board, "TM");
        assert_eq!(stored().service_filter, "", "the service of the board just left is dropped");
        assert_eq!(crate::web::storage::get_last_project().as_deref(), Some("TM"));
    }

    /// `persist` is called by every method that may have changed the record, so it has to be free when
    /// none did — and what is only on screen for now, which rows are unfolded, is not stored at all.
    #[test]
    fn nothing_is_written_when_nothing_changed() {
        let mut state = opened(&["RMS", "TM"]);
        state.set_env_filter("on:Prod".to_string());

        let before = crate::web::storage::storage_writes();

        state.set_env_filter("on:Prod".to_string());
        state.set_done_filter(IN_PROGRESS.to_string());
        state.set_service_filter(String::new());
        state.select("RMS".to_string());
        state.toggle("RMS-R1");
        state.board_pushed(Vec::new());

        assert_eq!(crate::web::storage::storage_writes(), before);

        // Opening the screen again on the board it was left on writes nothing either.
        let _ = opened(&["RMS", "TM"]);
        assert_eq!(crate::web::storage::storage_writes(), before);
    }

    /// A service is a service of one board. If the board was changed from another screen since, or is
    /// no longer one this reader can see, the remembered service would hide every release behind a
    /// choice the picker does not even list — so it is dropped, while the two filters that are about
    /// the reader rather than the board stay.
    #[test]
    fn a_remembered_service_is_honoured_only_on_its_own_board() {
        let mut state = opened(&["RMS", "TM"]);
        state.set_service_filter("rest-api".to_string());
        state.set_env_filter("on:Prod".to_string());

        // Home, in the meantime, moved this browser to another board.
        crate::web::storage::save_last_project("TM");

        let elsewhere = opened(&["RMS", "TM"]);
        assert_eq!(elsewhere.selected, "TM");
        assert!(elsewhere.service_filter.is_empty());
        assert_eq!(elsewhere.env_filter, "on:Prod");
        assert_eq!(stored().board, "TM", "and the record no longer claims the old board");

        // And a board that is gone from the list altogether.
        let mut state = opened(&["RMS", "TM"]);
        state.select("RMS".to_string());
        state.set_service_filter("rest-api".to_string());

        let without_it = opened(&["TM"]);
        assert_eq!(without_it.selected, "TM");
        assert!(without_it.service_filter.is_empty());
    }

    /// A record from a build that did not know a field still loads, and the state filter — the one whose
    /// absence is NOT "any" — reads as its default. A value that is not a record at all is a first visit.
    #[test]
    fn an_older_or_unreadable_record_still_opens_the_screen() {
        let older: ReleasesRecord = serde_json::from_str(r#"{"env_filter":"on:Prod"}"#).unwrap();

        assert_eq!(older.env_filter, "on:Prod");
        assert_eq!(older.done_filter, IN_PROGRESS);
        assert!(older.board.is_empty());

        let any: ReleasesRecord = serde_json::from_str(r#"{"done_filter":""}"#).unwrap();
        assert_eq!(any.done_filter, "");

        assert_eq!(ReleasesRecord::default().done_filter, IN_PROGRESS);
    }

    #[test]
    fn the_board_opened_is_the_remembered_one_when_it_can_still_be_seen() {
        let boards = [project("OLD", true), project("RMS", false), project("TM", false)];

        assert_eq!(board_to_open(&boards, Some("TM")), "TM");
        assert_eq!(board_to_open(&boards, Some("tm")), "TM", "in the project's own spelling");
        assert_eq!(
            board_to_open(&boards, Some("OLD")),
            "OLD",
            "an archived board reached before is still the one to open"
        );
        assert_eq!(board_to_open(&boards, Some("GONE")), "RMS", "else the first live one");
        assert_eq!(board_to_open(&boards, None), "RMS");
        assert_eq!(board_to_open(&[project("OLD", true)], None), "OLD", "else whatever there is");
        assert_eq!(board_to_open(&[], Some("TM")), "");
    }

    /// The choice outlives the visit now, so the service filtered by may be in no release today — and it
    /// must still be in the control, or the list is empty under a box saying "Any service".
    #[test]
    fn the_service_being_filtered_by_is_always_offered() {
        let releases = [release("RMS-R1", &["rest-api", "ui"])];

        assert_eq!(services_to_offer(&releases, ""), vec!["rest-api", "ui"]);
        assert_eq!(services_to_offer(&releases, "ui"), vec!["rest-api", "ui"]);
        assert_eq!(
            services_to_offer(&releases, "bridge"),
            vec!["bridge", "rest-api", "ui"]
        );
        assert_eq!(services_to_offer(&[], "bridge"), vec!["bridge"]);
    }

    /// "What is live" and "what has not got there yet" are the two questions the labels exist for, and
    /// between them they are every release — nothing is on neither list.
    #[test]
    fn a_release_is_on_one_side_of_an_environment_filter_or_the_other() {
        let staged = on("RMS-R1", &["Dev"]);
        let live = on("RMS-R2", &["Dev", "Prod"]);
        let nowhere = on("RMS-R3", &[]);

        assert!(matches_env_filter(&live, "on:Prod"));
        assert!(!matches_env_filter(&live, "not:Prod"));

        for release in [&staged, &nowhere] {
            assert!(!matches_env_filter(release, "on:Prod"));
            assert!(matches_env_filter(release, "not:Prod"));
        }

        // The label is found in any case, as everywhere else.
        assert!(matches_env_filter(&live, "on:prod"));
        assert!(matches_env_filter(&staged, "on:DEV"));

        for release in [&live, &staged, &nowhere] {
            assert!(matches_env_filter(release, ""), "no filter hides nothing");
            assert!(matches_env_filter(release, "a-value-from-a-newer-build"));
            assert!(matches_env_filter(release, "on:"), "a question about nothing is no question");
        }
    }

    /// "What still has somewhere to go" and "what is finished" are the two questions closing a release
    /// exists for, and between them they are every release — nothing is on neither list.
    #[test]
    fn a_release_is_either_still_going_out_or_closed() {
        let going_out = on("RMS-R1", &["Dev"]);

        let mut closed = on("RMS-R2", &["Dev", "Prod"]);
        closed.done_unix_seconds = Some(1_791_331_200);

        assert!(matches_done_filter(&going_out, IN_PROGRESS));
        assert!(!matches_done_filter(&going_out, DONE));

        assert!(matches_done_filter(&closed, DONE));
        assert!(!matches_done_filter(&closed, IN_PROGRESS));

        for release in [&going_out, &closed] {
            assert!(matches_done_filter(release, ""), "no filter hides nothing");
            assert!(matches_done_filter(release, "a-value-from-a-newer-build"));
        }
    }

    #[test]
    fn the_filter_offers_each_environment_once_in_order() {
        let releases = [
            on("RMS-R1", &["Prod", "Dev"]),
            on("RMS-R2", &["dev", "Stage"]),
            on("RMS-R3", &[]),
        ];

        assert_eq!(envs_of(&releases), vec!["Dev", "Prod", "Stage"]);
        assert!(envs_of(&[]).is_empty());
    }

    /// The filter follows the reader from board to board, so it can name an environment the board on
    /// screen has never used — and it must still be IN the control, or the list is empty with every
    /// filter apparently off.
    #[test]
    fn the_environment_being_filtered_by_is_always_offered() {
        let releases = [on("TM-R1", &["Dev"])];

        assert_eq!(envs_to_offer(&releases, ""), vec!["Dev"]);
        assert_eq!(envs_to_offer(&releases, "on:Prod"), vec!["Dev", "Prod"]);
        assert_eq!(envs_to_offer(&releases, "not:Prod"), vec!["Dev", "Prod"]);
        assert_eq!(
            envs_to_offer(&releases, "on:dev"),
            vec!["Dev"],
            "one this board does name is not offered twice"
        );
        assert_eq!(envs_to_offer(&[], "on:Prod"), vec!["Prod"]);

        // And the option for it reads as chosen however this board happens to case the label — but only
        // the one that asks the same question.
        assert!(filter_asks("on:Prod", ON_ENV, "prod"));
        assert!(!filter_asks("on:Prod", NOT_ON_ENV, "prod"));
        assert!(filter_asks("not:Prod", NOT_ON_ENV, "PROD"));
        assert!(!filter_asks("on:Prod", ON_ENV, "Pre-Prod"));
        assert!(!filter_asks("", ON_ENV, "Prod"));

        assert_eq!(env_of_filter("on:Pre-Prod"), Some("Pre-Prod"));
        assert_eq!(env_of_filter("not:Prod"), Some("Prod"));
        assert_eq!(env_of_filter(""), None);
        assert_eq!(env_of_filter("Prod"), None);
    }

    #[test]
    fn the_filter_offers_each_service_once_in_order() {
        let releases = [
            release("RMS-R1", &["rest-api", "ui"]),
            release("RMS-R2", &["rest-api"]),
            release("RMS-R3", &[]),
            release("RMS-R4", &["bridge"]),
        ];

        assert_eq!(microservices_of(&releases), vec!["bridge", "rest-api", "ui"]);
        assert!(microservices_of(&[]).is_empty());
    }

    #[test]
    fn a_release_is_shown_when_it_names_the_service_or_no_service_is_chosen() {
        let both = release("RMS-R1", &["rest-api", "ui"]);
        let none = release("RMS-R3", &[]);

        assert!(includes_service(&both, ""));
        assert!(includes_service(&none, ""), "no filter hides nothing");
        assert!(includes_service(&both, "ui"));
        assert!(!includes_service(&both, "bridge"));
        assert!(!includes_service(&none, "ui"));
    }

    /// The filter names a service of ONE project, so it must not follow the reader to the next board —
    /// where it would hide every release behind a choice the picker no longer even lists.
    #[test]
    fn changing_board_drops_what_was_about_the_old_one() {
        let mut state = opened(&["RMS", "TM"]);
        state.toggle("RMS-R1");
        state.set_service_filter("rest-api".to_string());

        // The same board is not a change, and nothing is thrown away for it.
        state.select("RMS".to_string());
        assert_eq!(state.service_filter, "rest-api");
        assert_eq!(state.expanded.len(), 1);

        state.set_env_filter("on:Prod".to_string());
        state.set_done_filter(IN_PROGRESS.to_string());

        state.select("TM".to_string());
        assert_eq!(state.selected, "TM");
        assert!(state.service_filter.is_empty());
        assert!(state.expanded.is_empty());
        assert_eq!(
            state.env_filter, "on:Prod",
            "\"what is live\" is how the reader wants releases shown, on any board"
        );
        assert_eq!(
            state.done_filter, IN_PROGRESS,
            "and so is \"what still has somewhere to go\""
        );
    }

    #[test]
    fn a_release_folds_and_unfolds_by_id() {
        let mut state = ComponentState::new();

        state.toggle("RMS-R1");
        state.toggle("RMS-R2");
        assert_eq!(state.expanded, vec!["RMS-R1", "RMS-R2"]);

        state.toggle("RMS-R1");
        assert_eq!(state.expanded, vec!["RMS-R2"]);
    }
}
