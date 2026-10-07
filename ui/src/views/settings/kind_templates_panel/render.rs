use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::kind_templates::KindTemplateResponse;

use crate::dialogs::DialogState;

/// Task-type templates — where a project's task types are actually configured.
///
/// One named set of types, followed by any number of projects, for the same reason columns work that way:
/// the vocabulary of work belongs to how a team works, not to one project.
#[component]
pub fn KindTemplatesPanel() -> Element {
    let data = use_signal(DataState::<Vec<KindTemplateResponse>>::default);

    render_table(data)
}

fn open(
    data: Signal<DataState<Vec<KindTemplateResponse>>>,
    template: Option<Rc<KindTemplateResponse>>,
) {
    let mut data = data;

    crate::dialogs::open(DialogState::EditKindTemplate {
        template,
        on_saved: EventHandler::new(move |_| data.write().reset()),
    });
}

fn render_table(data: Signal<DataState<Vec<KindTemplateResponse>>>) -> Element {
    let data_ra = data.read();

    let templates = match get_templates(data, &data_ra) {
        Ok(templates) => templates,
        Err(element) => return element,
    };

    rsx! {
        div { class: "page-header",
            h2 { class: "card-title", "Task-type templates" }
            div { class: "page-actions",
                button {
                    class: "btn btn-primary",
                    onclick: move |_| open(data, None),
                    "New template"
                }
            }
        }

        div { class: "field-hint",
            "A task type is optional on a task. A project picks its template in Projects setup; one that picks none simply has no types to choose from."
        }

        if templates.is_empty() {
            div { class: "empty-note", style: "margin-top: 12px",
                "No task-type templates yet. Create one, then assign it to a project."
            }
        } else {
            div { class: "table-responsive", style: "margin-top: 12px",
                table { class: "table",
                    thead {
                        tr {
                            th { "Name" }
                            th { "Description" }
                            th { "Task types" }
                            th { class: "num", "Projects" }
                            th { style: "width: 160px" }
                        }
                    }
                    tbody {
                        for template in templates.iter() {
                            RenderRow { key: "{template.id}", template: template.clone(), data }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn RenderRow(
    template: KindTemplateResponse,
    data: Signal<DataState<Vec<KindTemplateResponse>>>,
) -> Element {
    let template = Rc::new(template);

    let for_edit = template.clone();
    let id = template.id.clone();

    // Deleting a template in use is refused server-side, so the button is disabled here rather than
    // offering a click whose only outcome is an error message.
    let in_use = template.used_by > 0;

    rsx! {
        tr {
            td { "{template.name}" }
            td { class: "muted", "{template.description}" }
            td {
                if template.kinds.is_empty() {
                    span { class: "muted", "None" }
                } else {
                    for kind in template.kinds.iter() {
                        RenderKindTag {
                            key: "{kind.id}",
                            name: kind.name.clone(),
                            color: kind.color.clone(),
                            icon: kind.icon.clone(),
                        }
                    }
                }
            }
            td { class: "num", "{template.used_by}" }
            td {
                div { class: "btn-row",
                    button {
                        class: "btn btn-sm",
                        onclick: move |_| open(data, Some(for_edit.clone())),
                        "Edit"
                    }
                    button {
                        class: "btn btn-sm btn-danger",
                        disabled: in_use,
                        title: if in_use {
                            "Projects still follow this template — point them elsewhere first"
                        } else {
                            "Delete"
                        },
                        onclick: {
                            let id = id.clone();
                            move |_| {
                                let id = id.clone();
                                let mut data = data;
                                spawn(async move {
                                    match crate::api::delete_kind_template(&id).await {
                                        Ok(()) => data.write().reset(),
                                        Err(err) => crate::web::console_log(&err.message),
                                    }
                                });
                            }
                        },
                        "Delete"
                    }
                }
            }
        }
    }
}

/// A type as it appears on a sticker, so this table and the board agree on what one looks like.
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

fn get_templates(
    mut data: Signal<DataState<Vec<KindTemplateResponse>>>,
    data_ra: &DataState<Vec<KindTemplateResponse>>,
) -> Result<&[KindTemplateResponse], Element> {
    match data_ra.as_ref() {
        RenderState::None => {
            spawn(async move {
                data.write().set_loading();

                match crate::api::get_kind_templates().await {
                    // Admin-only, and a non-admin never reaches this section — an empty list is the
                    // honest reading of "not for you" rather than an invented error.
                    Ok(response) => data
                        .write()
                        .set_loaded(response.map(|itm| itm.templates).unwrap_or_default()),
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
        RenderState::Loaded(templates) => Ok(templates.as_slice()),
        RenderState::Error(err) => Err(rsx! {
            div { class: "error-banner", "{err}" }
        }),
    }
}
