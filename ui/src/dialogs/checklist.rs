use dioxus::prelude::*;
use task_manager_shared::subtasks::{SubtaskResponse, subtasks_progress};

/// Which items are open, by id.
///
/// A set rather than one open item: the checklist is read as a whole, and expanding the third to compare it
/// with the first is exactly what a reader does. Ids rather than positions, so a repaint arriving from the
/// WebSocket — which rebuilds the list — does not leave a different item open than the one that was clicked.
#[derive(Default)]
struct ComponentState {
    expanded: std::collections::HashSet<String>,
}

impl ComponentState {
    fn toggle(&mut self, id: &str) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.to_string());
        }
    }
}

/// The checklist of a task or of a goal, read-only.
///
/// One component for both, because it is one thing: the same list, the same shape, and the two dialogs are
/// already deliberately sharing a stylesheet.
///
/// **The title is what you see; the text is what you open.** Collapsed by default, and a click on a row
/// expands it — a checklist whose items were all open would be a wall of prose where the reader wanted a
/// list. An item with no text is drawn as a plain row rather than as a button, so there is no affordance
/// for an expansion that would show nothing.
///
/// Nothing here is editable: ticking an item is a change to the work, and every change to the work arrives
/// through `/mcp`. The tick is therefore drawn as a state, not as a control.
#[component]
pub fn Checklist(items: Vec<SubtaskResponse>) -> Element {
    let mut cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    let (done, total) = subtasks_progress(&items);

    rsx! {
        div { class: "task-view-checklist",
            div { class: "task-view-checklist-header",
                "Checklist"
                span { class: "board-column-count", "{done}/{total}" }
            }

            for item in items.iter() {
                {
                    let has_text = !item.text.trim().is_empty();
                    let expanded = has_text && cs_ra.expanded.contains(&item.id);
                    let row_class = if item.done { "task-view-check done" } else { "task-view-check" };

                    rsx! {
                        div { class: "{row_class}", key: "{item.id}",
                            if has_text {
                                button {
                                    class: "task-view-check-row",
                                    title: if expanded { "Collapse" } else { "Show the details" },
                                    onclick: {
                                        let id = item.id.clone();
                                        move |_| cs.write().toggle(&id)
                                    },
                                    // The caret is the whole of the affordance, and it points at what a
                                    // click does rather than at what the state is.
                                    span { class: "task-view-check-caret", if expanded { "▾" } else { "▸" } }
                                    span { class: "task-view-check-title", "{item.title}" }
                                    span { class: "task-view-check-box", if item.done { "✓" } }
                                }
                            } else {
                                div { class: "task-view-check-row",
                                    // An empty caret slot, so the titles of the items that do open and the
                                    // ones that do not still start on the same column.
                                    span { class: "task-view-check-caret" }
                                    span { class: "task-view-check-title", "{item.title}" }
                                    span { class: "task-view-check-box", if item.done { "✓" } }
                                }
                            }

                            if expanded {
                                // Markdown, escaped rather than trusted — the same call and the same reason
                                // as a task's text: an agent wrote it.
                                div {
                                    class: "task-view-check-text md",
                                    dangerous_inner_html: "{super::md_to_html(&item.text)}",
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
