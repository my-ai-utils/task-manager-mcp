use dioxus::prelude::*;
use task_manager_shared::projects::COLUMN_ID_DONE;
use task_manager_shared::task_title::{task_text_without_title, task_title};
use task_manager_shared::tasks::{
    FindTaskResponse, TaskGhActionResponse, TaskLinkResponse, TaskResponse,
};

/// One task, shown in full.
///
/// Read-only, like the board: every change to a task arrives through `/mcp`. What this owes the reader is the
/// whole card — text, attributes, thread — for a task that may not be on the board at all, because it belongs
/// to another project or was closed more than seven days ago.
///
/// **Laid out as four fixed areas, not as one scrolling document**: the text top left, the attributes in a
/// column of their own to its right, and the thread across the bottom half. Each half scrolls on its own, so
/// a task with forty comments still shows its text, and a task with a long text still shows that people have
/// been talking about it.
#[component]
pub fn ViewTaskDialog(found: FindTaskResponse) -> Element {
    let Some(task) = found.task.clone() else {
        // A miss carries the server's own message, which says what would fix it — a bad shape, an unknown
        // prefix with the known ones listed, or a number that is not on that board.
        let reason = if found.not_found.is_empty() {
            "Nothing found.".to_string()
        } else {
            found.not_found.clone()
        };

        return super::dialog_template_read_only(
            "Not found",
            rsx! {
                div { class: "empty-note", "{reason}" }
            },
            None,
        );
    };

    // The handle and the title, which is what a person calls this task when they talk about it. The project
    // was here instead and is now an attribute — it is the same project for every card you open off a board,
    // so it was paying for the one line that identifies the task.
    let title = format!("{} · {}", task.id, task_title(&task.text));
    let content = render_task(&task, &found);

    // Its own size class rather than `modal-lg`: this one takes 95% of the window. A dialog sized to its
    // content has no height for the two halves to be halves OF, and a task is the one thing on this side
    // worth the whole screen.
    //
    // No footer either — there is nothing to save, so there is nothing for Cancel to cancel.
    super::dialog_template_read_only(&title, content, Some("modal-task"))
}

/// "Closed today" / "Closed 3 days ago" — the age, not the timestamp.
///
/// Days rather than hours because the archive window is measured in days, so the number shown and the reason
/// the task will disappear are the same number.
fn closed_how_long_ago(closed_unix_seconds: i64) -> String {
    let now = js_sys::Date::now() as i64 / 1_000;
    let days = (now - closed_unix_seconds).max(0) / (24 * 60 * 60);

    match days {
        0 => "Closed today".to_string(),
        1 => "Closed yesterday".to_string(),
        _ => format!("Closed {days} days ago"),
    }
}

fn render_task(task: &TaskResponse, found: &FindTaskResponse) -> Element {
    // Without its first line: the header is already showing that line as the title, and a heading repeated
    // immediately under itself reads as a mistake.
    let body = task_text_without_title(&task.text);

    // Rendered rather than shown as source: agents write Markdown, and a checklist as literal dashes is
    // markedly harder to read. See `md_to_html` for which dialect and why raw HTML in it is harmless.
    let text_html = super::md_to_html(body);

    rsx! {
        div { class: "task-view",
            {render_goal_band(task)}
            div { class: "task-view-top",
                // The text and the checklist are one scrolling column, and the attribute column is the
                // other: the checklist is the breakdown OF the text, so it belongs under it and moves with
                // it. Wrapped even when there is no checklist, because the top area is a two-column grid
                // and a third child would drop onto a second row.
                div { class: "task-view-left",
                    if body.is_empty() {
                        // Said rather than left blank: an empty pane reads as something that failed to
                        // load, whereas most one-line tasks are one line on purpose.
                        div { class: "field-hint", "Nothing beyond the title." }
                    } else {
                        div { class: "task-view-text md", dangerous_inner_html: "{text_html}" }
                    }

                    // Nothing at all when there is no checklist, rather than an empty heading: most tasks
                    // have none, and a "Checklist 0/0" on every card would be noise on all of them.
                    if !task.subtasks.is_empty() {
                        super::Checklist { items: task.subtasks.clone() }
                    }
                }
                {render_attributes(task, found)}
            }
            {render_thread(task)}
        }
    }
}

/// Which epic this task belongs to, as the BAND ACROSS THE TOP — the same band the card has, in the same
/// colour, in the same place.
///
/// It was a row in the attribute column, which made it one labelled value among nine. That is the wrong shape
/// for it: a goal is not a property of the task like its type or its assignee, it is the FRAME the rest is
/// read inside — "todo" means one thing under "Crypto payments" and another on a task that stands alone. So it
/// arrives before the details rather than beside them, and a reader who opened the card off the board sees the
/// band they clicked, unmoved.
///
/// Nothing at all for a task with no goal: no band, no empty strip, and the layout below closes up — the two
/// halves are proportions of the dialog, and a row that is always there would take from them for nothing.
///
/// The handle beside the name: the name is what a person reads, the handle is what they type back into the
/// search box or hand to an agent.
fn render_goal_band(task: &TaskResponse) -> Element {
    let Some(goal) = task.goal.as_ref() else {
        return rsx! {};
    };

    // The goal's colour travels with the task, unlike the type's — so this band is right wherever the task was
    // looked up from, including a project that is not the one on screen.
    let goal_hex = task_manager_shared::kind_color::KindColor::parse_or_default(
        task.goal_color.as_deref().unwrap_or_default(),
    )
    .hex();

    let goal_title = task.goal_name.clone().unwrap_or_else(|| "Goal".to_string());

    rsx! {
        div { class: "task-view-goal-band", style: "background: {goal_hex}",
            span { class: "task-view-goal-id", "{goal}" }
            span { class: "task-view-goal-name", "{goal_title}" }
        }
    }
}

/// Everything about the task that is not its text, in one narrow column.
///
/// A column rather than a row of tags across the top: these are labelled values and there are eight of them
/// at most, which reads as a list. It also keeps them out of the text's way — the text is what the reader
/// came for and it gets the width.
fn render_attributes(task: &TaskResponse, found: &FindTaskResponse) -> Element {
    let status = if task.status == COLUMN_ID_DONE {
        "Done".to_string()
    } else {
        task.status.clone()
    };

    let priority = task_manager_shared::priority::Priority::parse_or_default(&task.priority);

    let assignee = task
        .assignee_name
        .clone()
        .or_else(|| task.assignee.clone())
        .unwrap_or_else(|| "Unassigned".to_string());

    rsx! {
        div { class: "task-view-attrs",
            // No Goal row here: the goal is the band across the top — see `render_goal_band`. It is the frame
            // the rest of these are read inside, not a tenth value in the list.
            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Status" }
                span { class: "tag", "{status}" }
            }

            // Always, unlike on a card — including Normal. A card leaves Normal out because its position in
            // the column already says it and a badge on every card is noise; the dialog is where somebody
            // comes to look a fact up, and a missing row there reads as "this build has no priorities" rather
            // than as "it is normal".
            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Priority" }
                span {
                    class: "sticker-priority",
                    style: "background: {priority.hex()}",
                    "{priority.title()}"
                }
            }

            // The type's id, uncoloured. Its colour and icon live on the project's type list, which this
            // response does not carry — and a lookup by id can land on a project that is not on screen, so
            // there is nothing to read them from. Better a plain tag than a wrong colour.
            if let Some(kind) = task.kind.as_ref() {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Type" }
                    span { class: "tag", "{kind}" }
                }
            }

            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Assignee" }
                div {
                    class: if task.assignee.is_some() { "" } else { "unassigned" },
                    "{assignee}"
                }
            }

            // Which board this is on. Worth saying because a dependency can name a task on ANOTHER project,
            // and following one is then the only way to notice you have left the board you came from.
            div { class: "task-view-attr",
                div { class: "task-view-attr-label", "Project" }
                div { "{found.project} · {found.project_name}" }
            }


            if task.blocked {
                div { class: "task-view-attr",
                    span { class: "sticker-blocked-flag", "Blocked" }
                }
            }

            // Said plainly, because this dialog is the one place a deleted task can still be opened — a
            // search found it — and a reader who was not told would take it for live work.
            if task.deleted_unix_seconds.is_some() {
                div { class: "task-view-attr",
                    span { class: "sticker-deleted-flag", "Deleted" }
                    div { class: "field-hint",
                        "Deleted, so it is off the board and out of every count. It is still here, and an agent can bring it back."
                    }
                }
            }

            // Before the labels: a document attached to a task is usually the specification the work is done
            // against, which is worth more to a reader than any tag on it. Ids rather than names — the board
            // snapshot carries the references and not the documents, so a path costs a request per card and
            // this screen deliberately does not make one until a row is clicked.
            if !task.documents.is_empty() {
                super::DocumentRefs { project: found.project.clone(), ids: task.documents.clone() }
            }

            // Straight after the documents, because the two are the same question asked in opposite
            // directions: a document is what the work was done against, a build is what came out of it. Most
            // tasks produced none and draw nothing at all — an empty "Builds" heading on every card would be
            // noise on all of them.
            if !task.gh_actions.is_empty() {
                {render_gh_actions(&task.gh_actions)}
            }

            if !task.labels.is_empty() {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Labels" }
                    div { class: "task-view-attr-tags",
                        for label in task.labels.iter() {
                            span { class: "tag", "{label}" }
                        }
                    }
                }
            }

            if !task.depends_on.is_empty() {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Waiting on" }
                    {render_links(&task.depends_on, &task.link_statuses)}
                }
            }

            if !task.blocks.is_empty() {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Blocking" }
                    {render_links(&task.blocks, &task.link_statuses)}
                }
            }

            // Only in Done, and only as an age. The exact timestamp says nothing a reader wants; how long ago
            // it closed is the same number as how close it is to leaving the board.
            if let Some(closed) = task.closed_unix_seconds.map(closed_how_long_ago) {
                div { class: "task-view-attr",
                    div { class: "task-view-attr-label", "Closed" }
                    div { title: "Work closed more than seven days ago leaves the board", "{closed}" }
                }
            }

            if found.archived {
                div { class: "field-hint",
                    "Closed more than seven days ago, so it is not on the board any more — it is still here, and still reachable by id."
                }
            }
        }
    }
}

/// The builds this task produced, oldest first.
///
/// **Real links, and the only thing on this screen that leaves the product.** A run lives on GitHub, so it
/// opens in a new tab: the board a person came from stays where it was, and the back button is not the way
/// back from a CI log. The name is what the row draws and the url is its tooltip — the url is what makes the
/// link work, not what anybody reads.
fn render_gh_actions(builds: &[TaskGhActionResponse]) -> Element {
    rsx! {
        div { class: "task-view-attr",
            div { class: "task-view-attr-label", "Builds" }
            div { class: "gh-actions",
                for build in builds.iter() {
                    a {
                        class: "gh-action",
                        key: "{build.url}",
                        href: "{build.url}",
                        target: "_blank",
                        rel: "noopener noreferrer",
                        title: "{build.url}",
                        span { class: "gh-action-title", "{build.title}" }
                        span { class: "gh-action-moment", "{attached_on(build.moment_unix_seconds)}" }
                    }
                }
            }
        }
    }
}

/// The day a build was recorded, as `2026-07-30`.
///
/// An absolute date rather than the age the Closed row shows, and for the opposite reason: a close date is
/// read to work out how long the card has left on the board, whereas a build is looked up long afterwards to
/// answer "which one was that" — where "112 days ago" is arithmetic the reader has to do themselves.
///
/// ISO order rather than the browser's locale, so a column of them sorts by eye and `03-04` is never
/// ambiguous.
fn attached_on(moment_unix_seconds: i64) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(
        (moment_unix_seconds * 1_000) as f64,
    ));

    format!(
        "{:04}-{:02}-{:02}",
        date.get_full_year(),
        // JavaScript counts months from zero, which is the one detail this function exists to get right.
        date.get_month() + 1,
        date.get_date()
    )
}

/// A list of task handles with what each one is doing, every handle a way into that task.
///
/// One per line rather than comma-separated: they are targets to hit with a mouse, and in a 200px column a
/// wrapped run of ids gives you no idea where one ends and the next begins.
///
/// The status is the reason a dependency is worth showing at all — "waiting on RMS-7" says nothing until you
/// know whether RMS-7 is done. Done is marked as such rather than merely named: it is the answer the reader is
/// scanning for, and the one that means this task is free to start.
fn render_links(ids: &[String], statuses: &[TaskLinkResponse]) -> Element {
    rsx! {
        div { class: "task-view-links",
            for id in ids.iter() {
                {
                    let status = statuses.iter().find(|itm| &itm.id == id).map(|itm| itm.status.clone());
                    let done = status.as_deref() == Some(COLUMN_ID_DONE);
                    rsx! {
                        div { class: "task-view-linked", key: "{id}",
                            button {
                                class: "task-view-link",
                                title: "Open {id}",
                                onclick: {
                                    let id = id.clone();
                                    move |_| show(id.clone())
                                },
                                "{id}"
                            }
                            // No status at all when the id names no task — a typo or a deleted blocker, which
                            // is exactly the case that keeps this task blocked. Better a visible gap than an
                            // invented column.
                            if let Some(status) = status {
                                span {
                                    class: if done { "task-view-link-status done" } else { "task-view-link-status" },
                                    if done { "done" } else { "{status}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Replace what this dialog is showing with another task.
///
/// The same window, deliberately: following a dependency is reading around the task you are on, not opening a
/// second thing — a stack of dialogs would bury the board under windows nobody asked for.
///
/// It goes through the server rather than the board that is loaded behind it, because a dependency can name a
/// task on another project or one closed long enough ago to be off the board entirely — the two cases where
/// the loaded board has no answer. A failure is shown the same way a missing id is: as the dialog's own
/// "Not found", carrying what went wrong.
fn show(id: String) {
    spawn(async move {
        let found = match crate::api::find_task(&id).await {
            Ok(found) => found,
            Err(err) => FindTaskResponse {
                task: None,
                goal: None,
                project: String::new(),
                project_name: String::new(),
                archived: false,
                not_found: err.message,
            },
        };

        crate::dialogs::open(crate::dialogs::DialogState::ViewTask { found });
    });
}

/// The bottom half: what people have said, oldest first.
fn render_thread(task: &TaskResponse) -> Element {
    rsx! {
        div { class: "task-view-bottom",
            div { class: "task-view-thread-header",
                "Comments"
                span { class: "board-column-count", "{task.comments.len()}" }
            }

            if task.comments.is_empty() {
                div { class: "field-hint", "No comments." }
            } else {
                div { class: "task-view-thread",
                    for (index , comment) in task.comments.iter().enumerate() {
                        div { class: "task-view-comment", key: "{index}",
                            div { class: "task-view-comment-who", "{comment.who}" }
                            div { class: "md", dangerous_inner_html: "{super::md_to_html(&comment.text)}" }
                        }
                    }
                }
            }
        }
    }
}
