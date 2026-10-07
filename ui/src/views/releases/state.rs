use dioxus_utils::DataState;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::releases::{ReleaseResponse, is_released_on_prod};

/// How many services a folded release names before the rest are counted rather than listed.
///
/// A row is one line. Three chips say which release this is for nearly every release there is — one
/// feature rarely touches more — and the count after them says there is more without widening the row.
pub const SERVICES_ON_A_ROW: usize = 3;

/// The two values the production filter can take besides "any", which is the empty string.
///
/// Strings rather than an enum because they are also the values of the `<option>`s that set them, and
/// one vocabulary for the control and the state is what keeps the two from drifting.
pub const ON_PROD: &str = "prod";
pub const NOT_ON_PROD: &str = "not-prod";

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
    /// Which releases are on screen by whether they have reached production: [`ON_PROD`],
    /// [`NOT_ON_PROD`], or empty for both. Deliberately NOT reset by `select`, unlike the service above:
    /// it is how this reader wants releases shown — "what is live" — and not something about one board.
    pub prod_filter: String,
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

    pub fn set_prod_filter(&mut self, value: String) {
        self.prod_filter = value;
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

/// Whether a release is on screen under the production filter. Anything that is not one of the two
/// values shows everything — an empty box and a value this build does not know are the same screen.
pub fn matches_prod_filter(release: &ReleaseResponse, filter: &str) -> bool {
    match filter {
        ON_PROD => is_released_on_prod(release),
        NOT_ON_PROD => !is_released_on_prod(release),
        _ => true,
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
                    datetime_unix_seconds: 0,
                    settings_update_note: String::new(),
                    description: String::new(),
                })
                .collect(),
            goals: Vec::new(),
            released_on_prod_unix_seconds: None,
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
        }
    }

    /// "What is live" and "what has not got there yet" are the two questions the mark exists for, and
    /// between them they are every release — nothing is on neither list.
    #[test]
    fn a_release_is_on_one_side_of_the_production_filter_or_the_other() {
        let staged = release("RMS-R1", &["rest-api"]);

        let mut live = release("RMS-R2", &["rest-api"]);
        live.released_on_prod_unix_seconds = Some(1_791_331_200);

        assert!(matches_prod_filter(&live, ON_PROD));
        assert!(!matches_prod_filter(&live, NOT_ON_PROD));

        assert!(!matches_prod_filter(&staged, ON_PROD));
        assert!(matches_prod_filter(&staged, NOT_ON_PROD));

        for release in [&live, &staged] {
            assert!(matches_prod_filter(release, ""), "no filter hides nothing");
            assert!(matches_prod_filter(release, "a-value-from-a-newer-build"));
        }
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

        state.set_prod_filter(ON_PROD.to_string());

        state.select("TM".to_string());
        assert_eq!(state.selected, "TM");
        assert!(state.service_filter.is_empty());
        assert!(state.expanded.is_empty());
        assert_eq!(
            state.prod_filter, ON_PROD,
            "\"what is live\" is how the reader wants releases shown, on any board"
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
