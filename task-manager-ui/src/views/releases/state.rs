use dioxus_utils::DataState;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::releases::ReleaseResponse;

/// How many services a folded release names before the rest are counted rather than listed.
///
/// A row is one line. Three chips say which release this is for nearly every release there is — one
/// feature rarely touches more — and the count after them says there is more without widening the row.
pub const SERVICES_ON_A_ROW: usize = 3;

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
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            deleted_unix_seconds: None,
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

        state.select("TM".to_string());
        assert_eq!(state.selected, "TM");
        assert!(state.service_filter.is_empty());
        assert!(state.expanded.is_empty());
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
