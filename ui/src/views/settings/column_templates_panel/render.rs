use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::column_templates::ColumnTemplateResponse;

use crate::dialogs::DialogState;

/// Column templates — where a board's columns are actually configured.
///
/// One named set of columns, followed by any number of projects. The indirection earns itself twice: the
/// projects on one board mostly share a workflow, so per-project columns meant typing the same four
/// columns into every project and watching them drift; and a template is a thing you change once and have
/// every project follow.
#[component]
pub fn ColumnTemplatesPanel() -> Element {
    let data = use_signal(DataState::<Vec<ColumnTemplateResponse>>::default);

    render_table(data)
}

fn open(
    data: Signal<DataState<Vec<ColumnTemplateResponse>>>,
    template: Option<Rc<ColumnTemplateResponse>>,
) {
    let mut data = data;

    crate::dialogs::open(DialogState::EditColumnTemplate {
        template,
        on_saved: EventHandler::new(move |_| data.write().reset()),
    });
}

fn render_table(data: Signal<DataState<Vec<ColumnTemplateResponse>>>) -> Element {
    let data_ra = data.read();

    let templates = match get_templates(data, &data_ra) {
        Ok(templates) => templates,
        Err(element) => return element,
    };

    rsx! {
        div { class: "page-header",
            h2 { class: "card-title", "Column templates" }
            div { class: "page-actions",
                button {
                    class: "btn btn-primary",
                    onclick: move |_| open(data, None),
                    "New template"
                }
            }
        }

        div { class: "field-hint",
            "Todo and Done are in every project and are not part of a template. A project picks its template in Projects setup; one that picks none has a board of just those two."
        }

        if templates.is_empty() {
            div { class: "empty-note", style: "margin-top: 12px",
                "No templates yet. Create one, then assign it to a project."
            }
        } else {
            div { class: "table-responsive", style: "margin-top: 12px",
                table { class: "table",
                    thead {
                        tr {
                            th { "Name" }
                            th { "Description" }
                            th { "Columns" }
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
    template: ColumnTemplateResponse,
    data: Signal<DataState<Vec<ColumnTemplateResponse>>>,
) -> Element {
    let template = Rc::new(template);

    let columns = template
        .columns
        .iter()
        .map(|itm| itm.name.as_str())
        .collect::<Vec<&str>>()
        .join(" → ");

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
                if columns.is_empty() {
                    span { class: "muted", "Todo → Done only" }
                } else {
                    span { class: "muted", "Todo → {columns} → Done" }
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
                                    match crate::api::delete_column_template(&id).await {
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

fn get_templates(
    mut data: Signal<DataState<Vec<ColumnTemplateResponse>>>,
    data_ra: &DataState<Vec<ColumnTemplateResponse>>,
) -> Result<&[ColumnTemplateResponse], Element> {
    match data_ra.as_ref() {
        RenderState::None => {
            spawn(async move {
                data.write().set_loading();

                match crate::api::get_column_templates().await {
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
