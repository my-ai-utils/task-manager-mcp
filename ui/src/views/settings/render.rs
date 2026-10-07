use dioxus::prelude::*;

use crate::AppRoute;
use crate::dialogs::DialogState;

/// Where a bare `/settings` lands.
const DEFAULT_SECTION: &str = SECTION_COLUMN_TEMPLATES;

const SECTION_COLUMN_TEMPLATES: &str = "column-templates";
const SECTION_KIND_TEMPLATES: &str = "task-type-templates";
const SECTION_DIAGNOSTICS: &str = "diagnostics";

/// Settings — a menu of areas on the left, the chosen one on the right.
///
/// The section comes off the route rather than out of a signal, so every area is linkable and the browser's
/// back button works between them: /settings/column-templates, /settings/diagnostics.
#[component]
pub fn RenderSettings() -> Element {
    let section = match use_route::<AppRoute>() {
        AppRoute::SettingsSection { section } => section,
        _ => DEFAULT_SECTION.to_string(),
    };

    // Bumped by an import, and the panels are keyed on it. A panel owns its own `DataState` and loads on
    // mount, so there is nothing here to reset — changing the key remounts it, which re-reads. That is why
    // this is a number rather than a signal handed down: the panels stay unaware they can be refreshed.
    let mut revision = use_signal(|| 0usize);
    let revision_now = *revision.read();

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Settings" }
            div { class: "page-actions",
                // A LINK rather than a button — a download is a navigation, and the session is a cookie
                // the browser attaches to one. Same reason the project export is a link.
                a {
                    class: "btn btn-sm",
                    title: "Download every column and task-type template as one YAML file",
                    href: "{task_manager_shared::templates_transfer::export_templates_url()}",
                    download: "",
                    "Export templates"
                }
                button {
                    class: "btn btn-sm",
                    title: "Apply a templates YAML file to this instance",
                    onclick: move |_| {
                        crate::dialogs::open(DialogState::ImportTemplates {
                            on_imported: EventHandler::new(move |_| {
                                revision.set(revision_now + 1);
                            }),
                        });
                    },
                    "Import templates"
                }
            }
        }

        div { class: "settings-layout",
            nav { class: "settings-menu",
                RenderMenuLink {
                    section: SECTION_COLUMN_TEMPLATES.to_string(),
                    title: "Column templates".to_string(),
                    active: section == SECTION_COLUMN_TEMPLATES,
                }
                RenderMenuLink {
                    section: SECTION_KIND_TEMPLATES.to_string(),
                    title: "Task-type templates".to_string(),
                    active: section == SECTION_KIND_TEMPLATES,
                }
                RenderMenuLink {
                    section: SECTION_DIAGNOSTICS.to_string(),
                    title: "Diagnostics".to_string(),
                    active: section == SECTION_DIAGNOSTICS,
                }
            }

            div { class: "settings-content",
                if section == SECTION_COLUMN_TEMPLATES {
                    super::ColumnTemplatesPanel { key: "{revision_now}" }
                } else if section == SECTION_KIND_TEMPLATES {
                    super::KindTemplatesPanel { key: "{revision_now}" }
                } else if section == SECTION_DIAGNOSTICS {
                    super::DiagnosticsPanel {}
                } else {
                    // A stale bookmark to a section that no longer exists lands here.
                    div { class: "empty-note",
                        "That settings section does not exist. Pick one on the left."
                    }
                }
            }
        }
    }
}

#[component]
fn RenderMenuLink(section: String, title: String, active: bool) -> Element {
    rsx! {
        Link {
            class: if active { "settings-menu-link active" } else { "settings-menu-link" },
            to: AppRoute::SettingsSection { section },
            "{title}"
        }
    }
}
