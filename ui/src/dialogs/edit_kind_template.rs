use std::rc::Rc;

use dioxus::prelude::*;
use rust_extensions::AsStr;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::kind_templates::{KindTemplateKind, KindTemplateResponse};

/// A task-type template, edited whole.
///
/// Same pattern as [`super::EditColumnTemplateDialog`]: model in, a draft built beside it, Save lit only
/// when they differ, the new model handed out through `on_submit`.
///
/// Called a "task type" on screen and a `kind` in the code, the API and the MCP tools. Deliberate: the
/// wire name is a contract with every agent already calling `tasks_create` with `kind`, and renaming it
/// to match a label would break them for nothing.
#[derive(Clone, PartialEq)]
struct ComponentState {
    original: Draft,
    draft: Draft,
    new_kind: NewKind,
}

/// The editable shape. `PartialEq` is the whole mechanism behind the Save button.
#[derive(Clone, PartialEq, Default)]
struct Draft {
    name: String,
    description: String,
    kinds: Vec<KindTemplateKind>,
}

#[derive(Clone, PartialEq)]
struct NewKind {
    id: String,
    name: String,
    description: String,
    color: String,
    icon: String,
}

impl Default for NewKind {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            description: String::new(),
            color: KindColor::default().as_str().to_string(),
            icon: String::new(),
        }
    }
}

impl ComponentState {
    fn new(template: Option<&KindTemplateResponse>) -> Self {
        let draft = match template {
            Some(template) => Draft {
                name: template.name.clone(),
                description: template.description.clone(),
                kinds: template.kinds.clone(),
            },
            None => Draft::default(),
        };

        Self {
            original: draft.clone(),
            draft,
            new_kind: NewKind::default(),
        }
    }

    fn is_changed(&self) -> bool {
        self.draft != self.original
    }

    /// Every type needs a name, checked here so an unsaveable set cannot be submitted and bounced.
    fn can_save(&self) -> bool {
        self.is_changed()
            && !self.draft.name.trim().is_empty()
            && self
                .draft
                .kinds
                .iter()
                .all(|itm| !itm.name.trim().is_empty())
    }

    fn can_add(&self) -> bool {
        let id = self.new_kind.id.trim();

        !id.is_empty() && !self.draft.kinds.iter().any(|itm| itm.id == id)
    }

    fn add(&mut self) {
        if !self.can_add() {
            return;
        }

        self.draft.kinds.push(KindTemplateKind {
            id: self.new_kind.id.trim().to_lowercase(),
            name: self.new_kind.name.trim().to_string(),
            description: self.new_kind.description.trim().to_string(),
            color: self.new_kind.color.clone(),
            icon: self.new_kind.icon.clone(),
        });

        // The colour and the icon are kept: picking them and then adding several is the usual way round.
        self.new_kind.id = String::new();
        self.new_kind.name = String::new();
        self.new_kind.description = String::new();
    }

    fn remove(&mut self, id: &str) {
        self.draft.kinds.retain(|itm| itm.id != id);
    }

    fn set_name(&mut self, id: &str, value: String) {
        if let Some(kind) = self.draft.kinds.iter_mut().find(|itm| itm.id == id) {
            kind.name = value;
        }
    }

    fn set_description(&mut self, id: &str, value: String) {
        if let Some(kind) = self.draft.kinds.iter_mut().find(|itm| itm.id == id) {
            kind.description = value;
        }
    }

    fn set_color(&mut self, id: &str, value: String) {
        if let Some(kind) = self.draft.kinds.iter_mut().find(|itm| itm.id == id) {
            kind.color = value;
        }
    }

    fn set_icon(&mut self, id: &str, value: String) {
        if let Some(kind) = self.draft.kinds.iter_mut().find(|itm| itm.id == id) {
            kind.icon = value;
        }
    }
}

/// What the dialog hands back: the complete template, ready to send.
#[derive(Clone, PartialEq)]
pub struct KindTemplateSubmit {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kinds: Vec<KindTemplateKind>,
}

#[component]
pub fn EditKindTemplateDialog(
    template: Option<Rc<KindTemplateResponse>>,
    on_submit: EventHandler<KindTemplateSubmit>,
) -> Element {
    let mut cs = use_signal(|| ComponentState::new(template.as_deref()));
    let cs_ra = cs.read();

    let id = template
        .as_deref()
        .map(|itm| itm.id.clone())
        .unwrap_or_default();

    let submit = move |_| {
        let id = id.clone();
        let draft = cs.read().draft.clone();

        on_submit.call(KindTemplateSubmit {
            id,
            name: draft.name.trim().to_string(),
            description: draft.description.trim().to_string(),
            kinds: draft.kinds,
        });
    };

    let name = cs_ra.draft.name.clone();
    let description = cs_ra.draft.description.clone();
    let draft = cs_ra.draft.kinds.clone();
    let used_by = template.as_deref().map(|itm| itm.used_by).unwrap_or(0);
    let new_kind = cs_ra.new_kind.clone();
    // The submit outcome is the router's, not this dialog's — see `DialogFeedback`.
    let feedback = super::feedback();
    let error = feedback.error.clone();
    let can_save = cs_ra.can_save() && !feedback.saving;
    let can_add = cs_ra.can_add();

    let content = rsx! {
        if !error.is_empty() {
            div { class: "error-banner", "{error}" }
        }

        if used_by > 1 {
            div { class: "field-hint",
                "{used_by} projects follow this template. Saving changes all of them."
            }
        }

        div { class: "form-row",
            label { "Name" }
            input {
                r#type: "text",
                placeholder: "Development",
                value: "{name}",
                oninput: move |event| cs.write().draft.name = event.value(),
            }
        }
        div { class: "form-row",
            label { "Description" }
            input {
                r#type: "text",
                value: "{description}",
                oninput: move |event| cs.write().draft.description = event.value(),
            }
        }

        div { class: "field-hint", style: "margin-top: 6px",
            "A task type is optional on a task. The description is what an agent reads before classifying one, so write the rule for applying it rather than a synonym of the name. Nothing is sent until Save."
        }

        if draft.is_empty() {
            div { class: "empty-note", style: "margin-top: 12px", "No task types yet." }
        } else {
            div { class: "table-responsive", style: "margin-top: 12px",
                table { class: "table",
                    thead {
                        tr {
                            th { "Id" }
                            th { "Name" }
                            th { "Description" }
                            th { style: "width: 210px", "Icon" }
                            th { style: "width: 150px", "Colour" }
                            th { style: "width: 90px" }
                        }
                    }
                    tbody {
                        for kind in draft.iter() {
                            tr { key: "{kind.id}",
                                td { class: "mono", "{kind.id}" }
                                td {
                                    input {
                                        r#type: "text",
                                        value: "{kind.name}",
                                        oninput: {
                                            let id = kind.id.clone();
                                            move |event: Event<FormData>| {
                                                cs.write().set_name(&id, event.value());
                                            }
                                        },
                                    }
                                }
                                td {
                                    input {
                                        r#type: "text",
                                        style: "width: 100%",
                                        value: "{kind.description}",
                                        oninput: {
                                            let id = kind.id.clone();
                                            move |event: Event<FormData>| {
                                                cs.write().set_description(&id, event.value());
                                            }
                                        },
                                    }
                                }
                                td {
                                    RenderIconPicker {
                                        value: kind.icon.clone(),
                                        on_pick: {
                                            let id = kind.id.clone();
                                            move |value| cs.write().set_icon(&id, value)
                                        },
                                    }
                                }
                                td {
                                    RenderColorPicker {
                                        value: kind.color.clone(),
                                        on_pick: {
                                            let id = kind.id.clone();
                                            move |value| cs.write().set_color(&id, value)
                                        },
                                    }
                                }
                                td {
                                    button {
                                        class: "btn btn-sm btn-danger",
                                        onclick: {
                                            let id = kind.id.clone();
                                            move |_| cs.write().remove(&id)
                                        },
                                        "Remove"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        div { class: "form-row-inline", style: "margin-top: 16px",
            div { class: "form-row",
                label { "Id" }
                input {
                    r#type: "text",
                    placeholder: "bug",
                    value: "{new_kind.id}",
                    oninput: move |event| cs.write().new_kind.id = event.value().to_lowercase(),
                }
            }
            div { class: "form-row",
                label { "Name" }
                input {
                    r#type: "text",
                    value: "{new_kind.name}",
                    oninput: move |event| cs.write().new_kind.name = event.value(),
                }
            }
            div { class: "form-row", style: "flex: 1 1 200px",
                label { "Description" }
                input {
                    r#type: "text",
                    value: "{new_kind.description}",
                    oninput: move |event| cs.write().new_kind.description = event.value(),
                }
            }
            div { class: "form-row",
                label { "Icon" }
                RenderIconPicker {
                    value: new_kind.icon.clone(),
                    on_pick: move |value| cs.write().new_kind.icon = value,
                }
            }
            div { class: "form-row",
                label { "Colour" }
                RenderColorPicker {
                    value: new_kind.color.clone(),
                    on_pick: move |value| cs.write().new_kind.color = value,
                }
            }
            button {
                class: "btn",
                disabled: !can_add,
                onclick: move |_| cs.write().add(),
                "Add task type"
            }
        }
    };

    let ok_button = rsx! {
        button { class: "btn btn-primary", disabled: !can_save, onclick: submit, "Save" }
    };

    let title = match template.as_deref() {
        Some(template) => format!("Task types · {}", template.name),
        None => "New task-type template".to_string(),
    };

    super::dialog_template_ex(&title, content, ok_button, Some("modal-xl"))
}

/// The icons that shipped in this bundle, drawn rather than named.
///
/// Same argument as the colour swatches: a picker that shows the thing beats one that spells it. The list
/// comes from `build.rs` reading `public/assets/images/task-icons`, so adding an icon is dropping an SVG
/// in that directory.
///
/// The first cell clears the choice — a type without an icon is normal, so "none" has to be reachable
/// rather than a state you can only leave.
#[component]
fn RenderIconPicker(value: String, on_pick: EventHandler<String>) -> Element {
    let picked = value.clone();

    rsx! {
        div { class: "icon-picker",
            button {
                r#type: "button",
                class: if picked.is_empty() { "icon-choice active" } else { "icon-choice" },
                title: "No icon",
                onclick: move |_| on_pick.call(String::new()),
                "—"
            }
            for name in crate::web::TASK_ICONS.iter() {
                button {
                    key: "{name}",
                    r#type: "button",
                    class: if picked == *name { "icon-choice active" } else { "icon-choice" },
                    title: "{name}",
                    onclick: {
                        let name = name.to_string();
                        move |_| on_pick.call(name.clone())
                    },
                    img { src: "{crate::web::icon_url(name)}", alt: "{name}" }
                }
            }
        }
    }
}

/// Swatches rather than a dropdown of colour names.
///
/// A colour picker that shows the colours is the obvious win over one that spells them, and the palette
/// is fixed and small enough to lay out in full.
#[component]
pub fn RenderColorPicker(value: String, on_pick: EventHandler<String>) -> Element {
    let picked = KindColor::parse_or_default(&value);

    // Built outside the markup: an rsx format string takes an identifier or a simple expression, not an
    // `if`, so the whole style is assembled first.
    let swatches: Vec<(KindColor, String)> = KindColor::ALL
        .iter()
        .map(|color| {
            let outline = if *color == picked {
                "#172b4d"
            } else {
                "transparent"
            };

            (
                *color,
                format!(
                    "width: 22px; height: 22px; border-radius: 4px; cursor: pointer; background: {}; border: 2px solid {outline};",
                    color.hex()
                ),
            )
        })
        .collect();

    rsx! {
        div { style: "display: flex; gap: 4px;",
            for (color , style) in swatches {
                button {
                    key: "{color.as_str()}",
                    r#type: "button",
                    title: "{color.title()}",
                    style: "{style}",
                    onclick: {
                        let value = color.as_str().to_string();
                        move |_| on_pick.call(value.clone())
                    },
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> KindTemplateResponse {
        KindTemplateResponse {
            id: "tpl".to_string(),
            name: "Development".to_string(),
            description: String::new(),
            kinds: vec![KindTemplateKind {
                id: "bug".to_string(),
                name: "Bug".to_string(),
                description: String::new(),
                color: "red".to_string(),
                icon: "bug".to_string(),
            }],
            used_by: 2,
        }
    }

    #[test]
    fn save_is_offered_only_once_something_differs() {
        let mut cs = ComponentState::new(Some(&template()));
        assert!(!cs.can_save());

        cs.set_name("bug", "Defect".to_string());
        assert!(cs.can_save());

        cs.set_name("bug", "Bug".to_string());
        assert!(!cs.is_changed());
    }

    /// A type with no name cannot be saved — caught here rather than after a round trip.
    #[test]
    fn a_nameless_type_blocks_saving() {
        let mut cs = ComponentState::new(Some(&template()));
        cs.set_name("bug", "  ".to_string());

        assert!(cs.is_changed());
        assert!(!cs.can_save());
    }

    /// So does a template with no name of its own.
    #[test]
    fn a_nameless_template_cannot_be_saved() {
        let mut cs = ComponentState::new(Some(&template()));
        cs.draft.name = "   ".to_string();

        assert!(cs.is_changed());
        assert!(!cs.can_save());
    }

    #[test]
    fn adding_is_local_and_refuses_a_duplicate_id() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.new_kind.id = "bug".to_string();
        assert!(!cs.can_add());

        cs.new_kind.id = " Feature ".to_string();
        cs.new_kind.name = " Feature ".to_string();
        assert!(cs.can_add());

        cs.add();

        assert_eq!(cs.draft.kinds.len(), 2);
        assert_eq!(cs.draft.kinds[1].id, "feature");
        assert_eq!(cs.draft.kinds[1].name, "Feature");
        assert!(cs.new_kind.id.is_empty());
        assert_eq!(
            cs.new_kind.color,
            KindColor::default().as_str(),
            "the picked colour survives an add"
        );

        // Choosing an icon is a change like any other, and it survives an add too.
        cs.set_icon("bug", "tech-debt".to_string());
        assert_eq!(cs.draft.kinds[0].icon, "tech-debt");
        assert!(cs.is_changed());
    }

    #[test]
    fn removing_everything_is_a_saveable_change() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.remove("bug");

        assert!(cs.draft.kinds.is_empty());
        assert!(cs.is_changed());
        assert!(cs.can_save(), "a template with no task types is legitimate");
    }

    /// A new template starts empty and needs a name before it can be saved.
    #[test]
    fn a_new_template_needs_a_name() {
        let mut cs = ComponentState::new(None);

        assert!(!cs.is_changed());
        assert!(!cs.can_save());

        cs.draft.name = "Development".to_string();
        assert!(cs.can_save());
    }
}
