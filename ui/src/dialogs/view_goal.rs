use dioxus::prelude::*;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::releases::moment_for_display;

/// Which of the two things under a goal's text is on screen.
///
/// Two tabs rather than two blocks one above the other, because they are two different readings of the
/// goal and each wants the room: the thread is WHY the work looks the way it does, the releases are WHAT
/// came of it. Stacked, a goal that had shipped pushed its own conversation off the bottom of the dialog.
#[derive(Clone, Copy, PartialEq, Debug)]
enum GoalTab {
    Comments,
    Releases,
}

/// What the dialog remembers while it is open.
struct ComponentState {
    tab: GoalTab,
    /// Which releases are unfolded, by id — ids rather than positions, for the reason the Releases screen
    /// keeps them that way.
    expanded: Vec<String>,
}

impl ComponentState {
    /// How a goal's dialog opens.
    ///
    /// **On the thread, unless there is nothing on it and there is something to show instead.** The
    /// thread matters most here — see [`ViewGoalDialog`] — so it is what a goal opens on; but a goal that
    /// has shipped and never been discussed would open on "nothing has been said", with the one thing
    /// there is to read a click away.
    ///
    /// **A single release is shown open, and several are folded.** One release is the answer to "what
    /// shipped for this", and making the reader unfold the only row there is would be a click for nothing.
    /// Several are a list to pick from, which is what a folded row is for.
    fn new(goal: &GoalResponse) -> Self {
        let tab = if goal.comments.is_empty() && !goal.releases.is_empty() {
            GoalTab::Releases
        } else {
            GoalTab::Comments
        };

        let expanded = match goal.releases.as_slice() {
            [only] => vec![only.id.clone()],
            _ => Vec::new(),
        };

        Self { tab, expanded }
    }

    fn select(&mut self, tab: GoalTab) {
        self.tab = tab;
    }

    fn toggle(&mut self, id: &str) {
        if let Some(at) = self.expanded.iter().position(|itm| itm == id) {
            self.expanded.remove(at);
        } else {
            self.expanded.push(id.to_string());
        }
    }
}

/// One goal, shown in full: what it is about, what has been said about it, and what it went out in.
///
/// Read-only, like every other view here — a goal is opened, renamed and closed through `/mcp`. Laid out as
/// the task dialog is, and deliberately with its classes rather than a parallel set: the two are the same
/// shape of thing (a text, some attributes, a thread), and two stylesheets for one shape drift.
///
/// The thread matters more here than on a task. A goal is where a conversation happens and tasks come out of
/// it, so `goals_add_comment` is the record of WHY the work under it looks the way it does — and until this
/// dialog existed there was nowhere in the browser to read it.
#[component]
pub fn ViewGoalDialog(goal: GoalResponse, status: String) -> Element {
    let title = format!("{} · {}", goal.id, goal.name);

    let cs = use_signal(|| ComponentState::new(&goal));

    let content = render_goal(&goal, &status, cs);

    // The same size class the task dialog uses: two halves need a height to be halves of, and a goal with a
    // long thread is exactly as worth the window as a task with one.
    super::dialog_template_read_only(&title, content, Some("modal-task"))
}

fn render_goal(goal: &GoalResponse, status: &str, cs: Signal<ComponentState>) -> Element {
    // Rendered rather than shown as source, and escaped rather than trusted — the same reason and the same
    // call as a task's text: agents write Markdown, and `md_to_html` escapes raw HTML instead of passing it
    // through.
    let description_html = super::md_to_html(&goal.description);
    let has_description = !goal.description.trim().is_empty();

    rsx! {
        // `with-tabs` is what sizes the two parts differently from a task's: the text takes what it needs
        // and no more, so a goal with two lines of description does not hold three fifths of the window
        // empty above a list of releases — see the stylesheet.
        div { class: "task-view with-tabs",
            div { class: "task-view-top",
                // One scrolling column for the description and the checklist under it, exactly as the task
                // dialog arranges the same two things — see the note there for why it is wrapped even when
                // there is no checklist.
                div { class: "task-view-left",
                    if has_description {
                        div { class: "task-view-text md", dangerous_inner_html: "{description_html}" }
                    } else {
                        // Said rather than left blank: a goal with no text is a goal whose shape lives in
                        // its thread, which is normal — the conversation comes first and the summary often
                        // never gets written.
                        div { class: "field-hint",
                            "No description. What this goal is about may be in the comments below."
                        }
                    }

                    if !goal.subtasks.is_empty() {
                        super::Checklist { items: goal.subtasks.clone() }
                    }
                }
                {render_attributes(goal, status)}
            }
            {render_tabs(goal, cs)}
        }
    }
}

/// The right-hand column: the goal's state, its progress, and its colour.
fn render_attributes(goal: &GoalResponse, status: &str) -> Element {
    let closed = goal.closed_unix_seconds.is_some();
    let hex = KindColor::parse_or_default(&goal.color).hex();
    let priority = task_manager_shared::priority::Priority::parse_or_default(&goal.priority);

    // The number the server counted, which includes archived work — not a count of anything on this screen.
    // A goal closes only once every task is done, so by then the oldest of them have aged off the board, and
    // a figure recomputed from a board read would report finished work as half-done.
    let progress = format!("{} of {} done", goal.done_amount, goal.tasks_amount);

    rsx! {
        div { class: "task-view-attrs",
            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Status" }
                span { class: "tag", "{status}" }
            }

            // Always, Normal included — the same reason as on the task dialog: this is the place a fact is
            // looked up, and a row that vanishes is ambiguous where a row saying "Normal" is not.
            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Priority" }
                span {
                    class: "sticker-priority",
                    style: "background: {priority.hex()}",
                    "{priority.title()}"
                }
            }

            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Progress" }
                div { "{progress}" }
            }

            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Colour" }
                // The swatch itself, not its name: the colour is what the board marks this goal's cards
                // with, and a word for it is one indirection more than a person needs.
                div { class: "task-view-goal", style: "background: {hex}",
                    span { class: "task-view-goal-id", "{goal.id}" }
                }
            }

            // A goal is where a decision is written down, so a document attached to one is usually the
            // decision itself — see the note in the task dialog for why these are ids and not paths.
            if !goal.documents.is_empty() {
                super::DocumentRefs { project: goal.project.clone(), ids: goal.documents.clone() }
            }

            // The newest one only, as a fact to look up beside the progress: "8 of 8 done" says the work
            // is finished and this says it is out, which are not the same thing. The releases themselves
            // are on their own tab below, where there is room to read them.
            if let Some(latest) = goal.releases.first() {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Released" }
                    div {
                        span { class: "goal-id", "{latest.id}" }
                        " {moment_for_display(latest.date_unix_seconds)}"
                    }
                }
            }

            if closed {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Closed" }
                    div {
                        "The resolution is the last comment below."
                    }
                }
            }
        }
    }
}

/// The lower part: the conversation and the releases, one at a time.
///
/// Both tabs are always there, with their counts — a tab that came and went with its content would move
/// the other one under the pointer, and `Releases 0` answers "has this shipped" as plainly as a release
/// would.
fn render_tabs(goal: &GoalResponse, mut cs: Signal<ComponentState>) -> Element {
    let cs_ra = cs.read();
    let tab = cs_ra.tab;

    let body = match tab {
        GoalTab::Comments => render_comments(goal),
        GoalTab::Releases => render_releases(goal, cs, &cs_ra.expanded),
    };

    rsx! {
        div { class: "task-view-bottom",
            div { class: "task-view-tabs",
                button {
                    class: if tab == GoalTab::Comments { "task-view-tab active" } else { "task-view-tab" },
                    onclick: move |_| cs.write().select(GoalTab::Comments),
                    "Comments"
                    span { class: "board-column-count", "{goal.comments.len()}" }
                }
                button {
                    class: if tab == GoalTab::Releases { "task-view-tab active" } else { "task-view-tab" },
                    onclick: move |_| cs.write().select(GoalTab::Releases),
                    "Releases"
                    span { class: "board-column-count", "{goal.releases.len()}" }
                }
            }

            {body}
        }
    }
}

/// The conversation, oldest first. The reason the work is shaped the way it is.
fn render_comments(goal: &GoalResponse) -> Element {
    if goal.comments.is_empty() {
        return rsx! {
            div { class: "field-hint", "Nothing has been said about this goal yet." }
        };
    }

    rsx! {
        div { class: "task-view-thread",
            for (index , comment) in goal.comments.iter().enumerate() {
                div { class: "task-view-comment", key: "{index}",
                    div { class: "task-view-comment-who", "{comment.who}" }
                    div { class: "md", dangerous_inner_html: "{super::md_to_html(&comment.text)}" }
                }
            }
        }
    }
}

/// The releases this goal went out in, newest first — the rows of the Releases screen, folded and opened
/// the same way.
///
/// Drawn with that screen's own row, so the two lists cannot come to disagree about what a release looks
/// like. Without the goal it shipped, though: here that is the dialog the reader is already in.
fn render_releases(
    goal: &GoalResponse,
    mut cs: Signal<ComponentState>,
    expanded: &[String],
) -> Element {
    if goal.releases.is_empty() {
        // Said, now that the tab is always there: most goals on screen are still being worked on, and an
        // empty tab with nothing in it would read as something that failed to load.
        return rsx! {
            div { class: "field-hint",
                "Nothing under this goal has been recorded as released yet."
            }
        };
    }

    rsx! {
        div { class: "task-view-releases",
            for release in goal.releases.iter() {
                super::ReleaseRow {
                    key: "{release.id}",
                    release: release.clone(),
                    open: expanded.contains(&release.id),
                    with_goals: false,
                    on_toggle: move |id: String| cs.write().toggle(&id),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use task_manager_shared::releases::ReleaseResponse;
    use task_manager_shared::tasks::TaskCommentResponse;

    use super::*;

    fn release(id: &str) -> ReleaseResponse {
        ReleaseResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            title: id.to_string(),
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

    fn goal(comments: usize, releases: &[&str]) -> GoalResponse {
        GoalResponse {
            id: "RMS-G7".to_string(),
            project: "RMS".to_string(),
            name: "Releases".to_string(),
            description: String::new(),
            color: String::new(),
            priority: String::new(),
            status: "todo".to_string(),
            tasks_amount: 0,
            done_amount: 0,
            documents: Vec::new(),
            subtasks: Vec::new(),
            releases: releases.iter().map(|id| release(id)).collect(),
            comments: (0..comments)
                .map(|index| TaskCommentResponse {
                    moment_unix_seconds: index as i64,
                    who: "AI".to_string(),
                    text: "a note".to_string(),
                })
                .collect(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            started_unix_seconds: None,
            closed_unix_seconds: None,
            deleted_unix_seconds: None,
        }
    }

    /// A goal opens on its thread — that is what the dialog is for — except when there is nothing on it
    /// and there IS something on the other tab: opening on "nothing has been said" with a release one
    /// click away would be the dialog hiding the only thing it has.
    #[test]
    fn a_goal_opens_on_its_thread_unless_only_the_releases_have_anything() {
        assert_eq!(ComponentState::new(&goal(2, &["RMS-R1"])).tab, GoalTab::Comments);
        assert_eq!(ComponentState::new(&goal(2, &[])).tab, GoalTab::Comments);
        assert_eq!(
            ComponentState::new(&goal(0, &[])).tab,
            GoalTab::Comments,
            "nothing anywhere is still a goal, and its thread is where it will be talked about"
        );
        assert_eq!(ComponentState::new(&goal(0, &["RMS-R1"])).tab, GoalTab::Releases);
    }

    /// One release is the answer and is shown open; several are a list and are folded, to be opened by
    /// the reader — and whichever were opened stay open across the tab being left and come back to.
    #[test]
    fn the_only_release_is_open_and_several_are_folded_until_asked_for() {
        assert_eq!(ComponentState::new(&goal(0, &["RMS-R1"])).expanded, vec!["RMS-R1"]);
        assert!(ComponentState::new(&goal(0, &[])).expanded.is_empty());

        let mut state = ComponentState::new(&goal(1, &["RMS-R3", "RMS-R1"]));
        assert!(state.expanded.is_empty());

        state.toggle("RMS-R1");
        state.toggle("RMS-R3");
        assert_eq!(state.expanded, vec!["RMS-R1", "RMS-R3"]);

        state.select(GoalTab::Releases);
        state.select(GoalTab::Comments);
        assert_eq!(state.expanded.len(), 2, "changing tab does not fold anything up");

        state.toggle("RMS-R1");
        assert_eq!(state.expanded, vec!["RMS-R3"]);
    }
}
