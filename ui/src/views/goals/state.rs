use dioxus_utils::DataState;
use serde::{Deserialize, Serialize};
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::tasks::TaskResponse;

use super::timeline::Month;

/// The two ways the Goals screen draws a board's goals.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum GoalsView {
    /// A row per goal, its tasks folded underneath — where somebody reads how each goal is going.
    #[default]
    List,
    /// A bar per goal over the days of one month — where somebody reads WHEN things were going on.
    Timeline,
}

impl GoalsView {
    /// The name it is stored under.
    pub fn key(self) -> &'static str {
        match self {
            Self::List => "list",
            Self::Timeline => "timeline",
        }
    }

    /// Back from a [`Self::key`]. Anything else is the list, which is what the screen was before there was
    /// a choice.
    fn parse(key: &str) -> Self {
        match key {
            "timeline" => Self::Timeline,
            _ => Self::List,
        }
    }
}

/// How the Goals screen was left — what this browser keeps of it, in `localStorage`.
///
/// The same arrangement as `ReleasesRecord`: [`ComponentState::new`] creates the state from it, once, and
/// [`ComponentState::persist`] is the only thing that writes it. A preference rather than a place in a
/// visit, hence `localStorage`. Every field has a default, so a record written by an older build loads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct GoalsRecord {
    /// [`GoalsView::key`].
    #[serde(default)]
    pub view: String,
}

#[derive(Default)]
pub struct ComponentState {
    pub projects: DataState<Vec<ProjectResponse>>,
    /// Which board is on screen, by PREFIX — the same vocabulary Home holds and the api speaks.
    pub selected: String,
    pub goals: DataState<Vec<GoalResponse>>,
    pub tasks: DataState<Vec<TaskResponse>>,
    /// Which groups are open. Kept across a repaint, so a push does not fold up what somebody was reading.
    pub expanded: Vec<String>,
    /// Whether closed goals are drawn. Off by default — what somebody opens this screen for is the work in
    /// flight. Deliberately NOT reset by `select`: it is how this reader wants goals shown, not something
    /// about one board.
    pub show_closed: bool,
    /// Which status is being looked at, by `GoalStatus::key` — empty for all of them. A preference of the
    /// reader's, like `show_closed`, so switching boards does not silently widen what is on screen.
    pub status_filter: String,
    /// Which goal's palette is open, if any. One at a time: two open palettes ask a question nobody asked.
    pub picking_color: Option<String>,
    /// List or timeline. Remembered by this browser — see [`GoalsRecord`].
    pub view: GoalsView,
    /// The month the timeline shows — `None` for the current one, which is what it opens on and what
    /// `Today` goes back to. Not reset by `select`: comparing two boards over the same month is the point.
    pub month: Option<Month>,
    /// Every goal of the board, closed ones past the archive window included — read only for the timeline,
    /// which is the one view that reaches back further than the live list does. Read once per board: a
    /// goal that has aged off the live list no longer changes, and the live ones come from the push.
    pub archive: DataState<Vec<GoalResponse>>,
    /// What storage holds now, so [`Self::persist`] can tell a change from choosing what is chosen.
    stored: Option<GoalsRecord>,
}

impl ComponentState {
    /// The state as this browser left it — the one read of storage this screen makes.
    pub fn new() -> Self {
        let stored = crate::web::storage::goals::get();
        let record = stored.clone().unwrap_or_default();

        Self {
            view: GoalsView::parse(&record.view),
            stored,
            ..Default::default()
        }
    }

    pub fn set_view(&mut self, view: GoalsView) {
        self.view = view;
        self.persist();
    }

    pub fn show_month(&mut self, month: Option<Month>) {
        self.month = month;
    }

    pub fn select(&mut self, prefix: String) {
        if self.selected == prefix {
            return;
        }

        self.selected = prefix;
        // Reset rather than clear: the next render sees `None` and loads, which is the same path a first
        // visit takes. See `get_goals`.
        self.goals.reset();
        self.tasks.reset();
        self.archive.reset();
        self.expanded.clear();
        self.picking_color = None;
    }

    pub fn toggle(&mut self, key: &str) {
        if let Some(at) = self.expanded.iter().position(|itm| itm == key) {
            self.expanded.remove(at);
        } else {
            self.expanded.push(key.to_string());
        }
    }

    /// The ONE place this state becomes a record.
    fn to_record(&self) -> GoalsRecord {
        GoalsRecord {
            view: self.view.key().to_string(),
        }
    }

    /// The only writer of the record, and it compares before it writes — choosing what is already chosen
    /// is not a write.
    fn persist(&mut self) {
        let record = self.to_record();

        if self.stored.as_ref() == Some(&record) {
            return;
        }

        crate::web::storage::goals::set(&record);
        self.stored = Some(record);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_visit_is_the_list_and_the_view_chosen_is_where_the_next_one_opens() {
        assert_eq!(ComponentState::new().view, GoalsView::List);

        let mut state = ComponentState::new();
        state.set_view(GoalsView::Timeline);

        assert_eq!(ComponentState::new().view, GoalsView::Timeline);
    }

    #[test]
    fn choosing_the_view_that_is_on_screen_is_not_a_write() {
        let mut state = ComponentState::new();
        state.set_view(GoalsView::Timeline);

        let writes = crate::web::storage::storage_writes();
        state.set_view(GoalsView::Timeline);
        state.show_month(None);

        assert_eq!(crate::web::storage::storage_writes(), writes);
    }

    #[test]
    fn a_view_this_build_does_not_know_is_the_list() {
        assert_eq!(GoalsView::parse("calendar"), GoalsView::List);
        assert_eq!(GoalsView::parse(""), GoalsView::List);
        assert_eq!(
            GoalsView::parse(GoalsView::Timeline.key()),
            GoalsView::Timeline
        );
    }
}
