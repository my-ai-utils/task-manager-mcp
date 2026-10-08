use dioxus_utils::DataState;
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

#[derive(Default)]
pub struct ComponentState {
    pub projects: DataState<Vec<ProjectResponse>>,
    /// Which board is on screen, by PREFIX — the same vocabulary Home and Goals hold and the api speaks.
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
}

impl ComponentState {
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
    }

    pub fn set_env_filter(&mut self, value: String) {
        self.env_filter = value;
    }

    pub fn set_done_filter(&mut self, value: String) {
        self.done_filter = value;
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
        let mut state = ComponentState {
            selected: "RMS".to_string(),
            expanded: vec!["RMS-R1".to_string()],
            service_filter: "rest-api".to_string(),
            ..Default::default()
        };

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
        let mut state = ComponentState::default();

        state.toggle("RMS-R1");
        state.toggle("RMS-R2");
        assert_eq!(state.expanded, vec!["RMS-R1", "RMS-R2"]);

        state.toggle("RMS-R1");
        assert_eq!(state.expanded, vec!["RMS-R2"]);
    }
}
