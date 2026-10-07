use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::projects::ProjectResponse;

use crate::dialogs::DialogState;

/// Projects setup: every project in one table, edited through dialogs.
///
/// A table rather than a dropdown-plus-four-cards, which is what this was: the dropdown hid every project
/// but one, so "which prefixes are taken" and "which project has nobody on it" were questions you had to
/// click through the list to answer. They are now columns.
#[component]
pub fn RenderProjectsSetup() -> Element {
    let data = use_signal(DataState::<Vec<ProjectResponse>>::default);
    // Client state, not a second request: the list already arrives carrying `archived` on every project, so
    // the toggle filters what is already in hand and flips without a round trip. It also means an archive
    // press and this toggle cannot disagree — they read the same `Vec`.
    let show_archived = use_signal(|| false);

    render_table(data, show_archived)
}

/// Opening the editor and reloading afterwards are the same two lines everywhere, so they live in one
/// place. `data.write().reset()` puts the `DataState` back to `None`, and the next render re-reads —
/// the server is the only thing that says what was actually stored.
fn open(
    data: Signal<DataState<Vec<ProjectResponse>>>,
    make: impl FnOnce(EventHandler<()>) -> DialogState,
) {
    let mut data = data;

    crate::dialogs::open(make(EventHandler::new(move |_| data.write().reset())));
}

fn render_table(
    data: Signal<DataState<Vec<ProjectResponse>>>,
    show_archived: Signal<bool>,
) -> Element {
    let data_ra = data.read();

    let projects = match get_projects(data, &data_ra) {
        Ok(projects) => projects,
        Err(element) => return element,
    };

    let show = *show_archived.read();

    // Narrowed before the rsx rather than inside the loop, so the "nothing here" note below reads the list
    // that is actually drawn — filtering in the loop would leave a table with a header and no rows.
    let visible: Vec<&ProjectResponse> = projects
        .iter()
        .filter(|itm| show || !itm.archived)
        .collect();

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Projects setup" }
            div { class: "page-actions",
                button {
                    class: "btn",
                    title: "Archived projects are hidden from every project picker. This screen is the one place they can be seen and brought back",
                    // The label carries the state rather than a pressed style: there is no `.btn.active` rule
                    // in the stylesheet, and the source/rendered switch on Documents already reads this way.
                    onclick: move |_| {
                        let mut show_archived = show_archived;
                        let now = *show_archived.read();
                        show_archived.set(!now);
                    },
                    if show { "Hide archived" } else { "Show archived" }
                }
                button {
                    class: "btn btn-primary",
                    onclick: move |_| {
                        open(data, |on_saved| DialogState::EditProject { project: None, on_saved });
                    },
                    "New project"
                }
            }
        }

        if visible.is_empty() {
            // Two different facts, and telling them apart is the point: a first run with nothing in it, and a
            // board full of projects that have all been put away. The second one has a way out on screen.
            if projects.is_empty() {
                div { class: "empty-note", "No projects yet. Create the first one." }
            } else {
                div { class: "empty-note", "Every project here is archived. Press Show archived to see them." }
            }
        } else {
            div { class: "table-responsive",
                table { class: "table",
                    thead {
                        tr {
                            th { "Prefix" }
                            th { "Name" }
                            th { "Description" }
                            th { class: "num", "Tasks" }
                            th { "Column template" }
                            th { "Task-type template" }
                            th { class: "num", "Members" }
                            th { style: "width: 340px" }
                        }
                    }
                    tbody {
                        for project in visible.iter() {
                            // Keyed by prefix: it is unique, it is what this side knows a project by, and it
                            // is stable for as long as the row is — a rename re-reads the whole list anyway.
                            RenderRow { key: "{project.prefix}", project: (*project).clone(), data }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenderRow(project: ProjectResponse, data: Signal<DataState<Vec<ProjectResponse>>>) -> Element {
    // Shared by all four buttons, and cheap: an `Rc` is what the dialog state carries anyway.
    let project = Rc::new(project);

    // The columns a project actually configured, in order — Todo and Done are left out because every
    // project has them and listing them in every row would say nothing.
    let mut ordered = project.columns.clone();
    ordered.sort_by_key(|itm| itm.order);

    let columns = ordered
        .iter()
        .map(|itm| itm.name.as_str())
        .collect::<Vec<&str>>()
        .join(" → ");

    let for_edit = project.clone();
    let for_members = project.clone();

    // Composed by the shared crate, so this side and the route it points at cannot drift — there is no
    // request builder in between to keep them honest.
    let export_url = task_manager_shared::project_transfer::export_project_url(&project.prefix);

    rsx! {
        tr {
            td { class: "mono",
                "{project.prefix}"
                // Only ever visible with the toggle on, since an archived row is not drawn otherwise — so
                // it is the answer to "which of these did I put away", not a decoration on a normal table.
                if project.archived {
                    span {
                        class: "tag",
                        style: "margin-left: 6px",
                        title: "Put away: hidden from every project picker, and still reachable by link",
                        "archived"
                    }
                }
            }
            td { "{project.name}" }
            td { class: "muted", "{project.description}" }
            td { class: "num", "{project.tasks_amount}" }
            td {
                // The template's NAME, then what it resolves to. Which template a project follows is the
                // thing you change; the column list is the thing you check afterwards.
                if let Some(template) = project.column_template_name.as_ref() {
                    div { "{template}" }
                    div { class: "field-hint", "{columns}" }
                } else {
                    span { class: "muted", "Todo → Done" }
                }
            }
            td {
                // The template's name, then the types it resolves to — same reading as the column cell.
                if let Some(template) = project.kind_template_name.as_ref() {
                    div { "{template}" }
                    div {
                        for kind in project.kinds.iter() {
                            RenderKindTag {
                                key: "{kind.id}",
                                name: kind.name.clone(),
                                color: kind.color.clone(),
                                icon: kind.icon.clone(),
                            }
                        }
                    }
                } else {
                    span { class: "muted", "—" }
                }
            }
            td { class: "num", "{project.members.len()}" }
            td {
                div { class: "btn-row",
                    button {
                        class: "btn btn-sm",
                        onclick: move |_| {
                            let project = for_edit.clone();
                            open(data, |on_saved| DialogState::EditProject { project: Some(project), on_saved });
                        },
                        "Edit"
                    }
                    button {
                        class: "btn btn-sm",
                        onclick: move |_| {
                            let project = for_members.clone();
                            open(data, |on_saved| DialogState::EditMembers { project, on_saved });
                        },
                        "Members"
                    }
                    button {
                        class: "btn btn-sm",
                        title: "Mirror a GitHub repository into this project's documents",
                        onclick: {
                            let prefix = project.prefix.clone();

                            move |_| {
                                let prefix = prefix.clone();
                                // Prefix rather than the `Rc<ProjectResponse>` every other dialog here
                                // takes: this one reads its own list from the server, and a project's
                                // connections are not on the project response at all — the live half of
                                // each of them is in the service's memory, not on the row.
                                open(data, |on_saved| DialogState::GithubConnections {
                                    project: prefix,
                                    revision: 0,
                                    on_saved,
                                });
                            }
                        },
                        "GitHub"
                    }
                    // A LINK rather than a button, and the only one on this screen. A download is a
                    // navigation: the browser asks for the url, sees `Content-Disposition: attachment` and
                    // saves the file without leaving the page — and the session rides along, because it is a
                    // cookie. Doing it through `fetch` would mean holding the whole archive in the wasm heap
                    // to hand it back to the browser, which is the one thing the streaming endpoint exists to
                    // avoid.
                    a {
                        class: "btn btn-sm",
                        title: "Download this whole project as a zip — tasks, goals, comments and documents",
                        href: "{export_url}",
                        download: "",
                        "Export"
                    }
                    button {
                        class: "btn btn-sm",
                        title: "Pour an exported project into this one",
                        onclick: {
                            let prefix = project.prefix.clone();
                            let name = project.name.clone();

                            move |_| {
                                let prefix = prefix.clone();
                                let name = name.clone();

                                open(data, |on_imported| DialogState::ImportProject {
                                    project: prefix,
                                    project_name: name,
                                    on_imported,
                                });
                            }
                        },
                        "Import"
                    }
                    // No confirm step, and that is the point of a soft delete: nothing is destroyed, the
                    // same button puts it back, and a dialog in front of a reversible act only teaches
                    // people to click through dialogs.
                    button {
                        class: if project.archived { "btn btn-sm" } else { "btn btn-sm btn-danger" },
                        title: if project.archived {
                            "Bring this project back into the pickers"
                        } else {
                            "Put this project away. Nothing is deleted: its links keep working, its tasks and documents stay, and it keeps holding its prefix"
                        },
                        onclick: {
                            let prefix = project.prefix.clone();
                            let archived = project.archived;

                            move |_| {
                                let prefix = prefix.clone();
                                let mut data = data;

                                spawn(async move {
                                    match crate::api::set_project_archived(&prefix, !archived).await {
                                        // Re-read rather than patch the row in place: the server is the only
                                        // thing that says what was actually stored, which is the same rule
                                        // every dialog on this screen already follows.
                                        Ok(()) => data.write().reset(),
                                        Err(err) => crate::dialogs::open(DialogState::Message {
                                            title: "Could not change that".to_string(),
                                            text: err.message,
                                        }),
                                    }
                                });
                            }
                        },
                        if project.archived { "Unarchive" } else { "Archive" }
                    }
                }
            }
        }
    }
}

/// A kind as it appears on a sticker, so the table and the board agree on what a kind looks like.
#[component]
fn RenderKindTag(name: String, color: String, icon: String) -> Element {
    let hex = KindColor::parse_or_default(&color).hex();

    rsx! {
        span { class: "sticker-kind", style: "background: {hex}; margin-right: 4px",
            if crate::web::icon_exists(&icon) {
                img { class: "sticker-kind-icon", src: "{crate::web::icon_url(&icon)}", alt: "" }
            }
            "{name}"
        }
    }
}

fn get_projects(
    mut data: Signal<DataState<Vec<ProjectResponse>>>,
    data_ra: &DataState<Vec<ProjectResponse>>,
) -> Result<&[ProjectResponse], Element> {
    match data_ra.as_ref() {
        RenderState::None => {
            spawn(async move {
                data.write().set_loading();

                match crate::api::get_projects().await {
                    Ok(response) => data.write().set_loaded(response.projects),
                    Err(err) => data.write().set_error(err.message),
                }
            });

            Err(rsx! {
                div { class: "loading-note", "Loading…" }
            })
        }
        RenderState::Loading => Err(rsx! {
            div { class: "loading-note", "Loading…" }
        }),
        RenderState::Loaded(projects) => Ok(projects.as_slice()),
        RenderState::Error(err) => Err(rsx! {
            div { class: "error-banner", "{err}" }
        }),
    }
}
