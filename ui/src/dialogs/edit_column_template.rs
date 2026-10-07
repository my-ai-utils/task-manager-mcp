use std::rc::Rc;

use dioxus::prelude::*;
use task_manager_shared::column_templates::{ColumnTemplateColumn, ColumnTemplateResponse};

/// A column template, edited whole.
///
/// **The dialog pattern, in one place.** A model goes in; editing builds a *new* model beside it; Save
/// lights up only when the two differ; pressing it hands the new model out through `on_submit` and the
/// caller does the request and the refresh. Nothing here calls an API and nothing here touches the page's
/// state.
///
/// What that buys, concretely: adding a column is a local edit, so a half-finished set is never visible
/// to anyone and never reaches the server; Cancel costs nothing because nothing was sent; and one request
/// replaces the previous three (add, update, delete) with no ordering to get wrong.
#[derive(Clone, PartialEq)]
struct ComponentState {
    /// What arrived. Never edited — it is the thing `is_changed` compares against.
    original: Draft,
    draft: Draft,
    new_column: NewColumn,
}

/// The editable shape of a template. `PartialEq` is the whole mechanism behind the Save button.
#[derive(Clone, PartialEq)]
struct Draft {
    name: String,
    description: String,
    columns: Vec<ColumnTemplateColumn>,
}

#[derive(Clone, PartialEq, Default)]
struct NewColumn {
    id: String,
    name: String,
    description: String,
    order: String,
}

impl ComponentState {
    fn new(template: Option<&ColumnTemplateResponse>) -> Self {
        let draft = match template {
            Some(template) => Draft {
                name: template.name.clone(),
                description: template.description.clone(),
                columns: template.columns.clone(),
            },
            None => Draft {
                name: String::new(),
                description: String::new(),
                columns: Vec::new(),
            },
        };

        Self {
            original: draft.clone(),
            draft,
            new_column: NewColumn {
                order: "10".to_string(),
                ..Default::default()
            },
        }
    }

    /// Save is offered only when there is something to save.
    ///
    /// A disabled Save on an untouched form is the honest state: it says "nothing here differs" without
    /// making you press a button to find out.
    fn is_changed(&self) -> bool {
        self.draft != self.original
    }

    fn can_save(&self) -> bool {
        self.is_changed() && !self.draft.name.trim().is_empty()
    }

    fn can_add_column(&self) -> bool {
        let id = self.new_column.id.trim();

        !id.is_empty() && !self.draft.columns.iter().any(|itm| itm.id == id)
    }

    /// Add to the DRAFT. No request — that happens once, on Save.
    fn add_column(&mut self) {
        if !self.can_add_column() {
            return;
        }

        let order = self.new_column.order.trim().parse::<i32>().unwrap_or(10);

        self.draft.columns.push(ColumnTemplateColumn {
            id: self.new_column.id.trim().to_lowercase(),
            name: self.new_column.name.trim().to_string(),
            description: self.new_column.description.trim().to_string(),
            order,
        });

        self.draft.columns.sort_by_key(|itm| itm.order);

        // Cleared so the row is ready for the next one, and the order kept — columns are usually added
        // in sequence, so the last value is the better default.
        self.new_column.id = String::new();
        self.new_column.name = String::new();
        self.new_column.description = String::new();
    }

    fn remove_column(&mut self, id: &str) {
        self.draft.columns.retain(|itm| itm.id != id);
    }

    fn set_column_name(&mut self, id: &str, value: String) {
        if let Some(column) = self.draft.columns.iter_mut().find(|itm| itm.id == id) {
            column.name = value;
        }
    }

    fn set_column_description(&mut self, id: &str, value: String) {
        if let Some(column) = self.draft.columns.iter_mut().find(|itm| itm.id == id) {
            column.description = value;
        }
    }

    /// An unparseable order leaves the value alone rather than resetting it to something the person did
    /// not type — mid-typing "1" on the way to "15" must not be fought.
    fn set_column_order(&mut self, id: &str, value: String) {
        if let Ok(order) = value.trim().parse::<i32>()
            && let Some(column) = self.draft.columns.iter_mut().find(|itm| itm.id == id)
        {
            column.order = order;
        }
    }
}

/// What the dialog hands back: the complete template, ready to send.
#[derive(Clone, PartialEq)]
pub struct ColumnTemplateSubmit {
    pub id: String,
    pub name: String,
    pub description: String,
    pub columns: Vec<ColumnTemplateColumn>,
}

#[component]
pub fn EditColumnTemplateDialog(
    template: Option<Rc<ColumnTemplateResponse>>,
    on_submit: EventHandler<ColumnTemplateSubmit>,
) -> Element {
    let mut cs = use_signal(|| ComponentState::new(template.as_deref()));
    let cs_ra = cs.read();

    let id = template
        .as_deref()
        .map(|itm| itm.id.clone())
        .unwrap_or_default();

    let submit = move |_| {
        let id = id.clone();
        let ra = cs.read();
        let draft = ra.draft.clone();
        drop(ra);

        on_submit.call(ColumnTemplateSubmit {
            id,
            name: draft.name.trim().to_string(),
            description: draft.description.trim().to_string(),
            columns: draft.columns,
        });
    };

    let title = match template.as_deref() {
        Some(template) => format!("Column template · {}", template.name),
        None => "New column template".to_string(),
    };

    // Said out loud when it is more than one, because editing the template moves every one of them at
    // once and that is not obvious from inside a dialog about columns.
    let used_by = template.as_deref().map(|itm| itm.used_by).unwrap_or(0);

    let name = cs_ra.draft.name.clone();
    let description = cs_ra.draft.description.clone();
    let columns = cs_ra.draft.columns.clone();
    let new_column = cs_ra.new_column.clone();
    // The submit outcome is the router's, not this dialog's — see `DialogFeedback`.
    let feedback = super::feedback();
    let error = feedback.error.clone();
    let can_save = cs_ra.can_save() && !feedback.saving;
    let can_add = cs_ra.can_add_column();

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
            "Todo and Done are in every project and are not listed here. A column id is typed once and never renamed — tasks point at it as their status. Nothing is sent until Save."
        }

        div { class: "table-responsive", style: "margin-top: 12px",
            table { class: "table",
                thead {
                    tr {
                        th { style: "width: 80px", "Order" }
                        th { "Id" }
                        th { "Name" }
                        th { "Description" }
                        th { style: "width: 90px" }
                    }
                }
                tbody {
                    tr {
                        td { class: "muted", "first" }
                        td { class: "mono", "todo" }
                        td { "Todo" }
                        td { class: "muted", "Always present" }
                        td {}
                    }
                    for column in columns.iter() {
                        tr { key: "{column.id}",
                            td {
                                input {
                                    r#type: "text",
                                    style: "width: 70px",
                                    value: "{column.order}",
                                    oninput: {
                                        let id = column.id.clone();
                                        move |event: Event<FormData>| {
                                            cs.write().set_column_order(&id, event.value());
                                        }
                                    },
                                }
                            }
                            td { class: "mono", "{column.id}" }
                            td {
                                input {
                                    r#type: "text",
                                    value: "{column.name}",
                                    oninput: {
                                        let id = column.id.clone();
                                        move |event: Event<FormData>| {
                                            cs.write().set_column_name(&id, event.value());
                                        }
                                    },
                                }
                            }
                            td {
                                input {
                                    r#type: "text",
                                    style: "width: 100%",
                                    value: "{column.description}",
                                    oninput: {
                                        let id = column.id.clone();
                                        move |event: Event<FormData>| {
                                            cs.write().set_column_description(&id, event.value());
                                        }
                                    },
                                }
                            }
                            td {
                                button {
                                    class: "btn btn-sm btn-danger",
                                    onclick: {
                                        let id = column.id.clone();
                                        move |_| cs.write().remove_column(&id)
                                    },
                                    "Remove"
                                }
                            }
                        }
                    }
                    tr {
                        td { class: "muted", "last" }
                        td { class: "mono", "done" }
                        td { "Done" }
                        td { class: "muted", "Always present. A dependency counts as satisfied only here" }
                        td {}
                    }
                }
            }
        }

        div { class: "form-row-inline", style: "margin-top: 16px",
            div { class: "form-row",
                label { "Order" }
                input {
                    r#type: "text",
                    style: "width: 70px",
                    value: "{new_column.order}",
                    oninput: move |event| cs.write().new_column.order = event.value(),
                }
            }
            div { class: "form-row",
                label { "Id" }
                input {
                    r#type: "text",
                    placeholder: "in-progress",
                    value: "{new_column.id}",
                    oninput: move |event| {
                        cs.write().new_column.id = event.value().to_lowercase()
                    },
                }
            }
            div { class: "form-row",
                label { "Name" }
                input {
                    r#type: "text",
                    value: "{new_column.name}",
                    oninput: move |event| cs.write().new_column.name = event.value(),
                }
            }
            div { class: "form-row", style: "flex: 1 1 200px",
                label { "Description" }
                input {
                    r#type: "text",
                    value: "{new_column.description}",
                    oninput: move |event| cs.write().new_column.description = event.value(),
                }
            }
            button {
                class: "btn",
                disabled: !can_add,
                onclick: move |_| cs.write().add_column(),
                "Add column"
            }
        }
    };

    let ok_button = rsx! {
        button { class: "btn btn-primary", disabled: !can_save, onclick: submit, "Save" }
    };

    super::dialog_template_ex(&title, content, ok_button, Some("modal-xl"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template() -> ColumnTemplateResponse {
        ColumnTemplateResponse {
            id: "tpl".to_string(),
            name: "Development".to_string(),
            description: String::new(),
            columns: vec![ColumnTemplateColumn {
                id: "in-progress".to_string(),
                name: "In progress".to_string(),
                description: String::new(),
                order: 10,
            }],
            used_by: 2,
        }
    }

    /// The Save button's whole contract: nothing to save until something differs.
    #[test]
    fn save_is_offered_only_once_something_differs() {
        let mut cs = ComponentState::new(Some(&template()));
        assert!(!cs.is_changed());
        assert!(!cs.can_save());

        cs.draft.name = "Dev".to_string();
        assert!(cs.is_changed());
        assert!(cs.can_save());

        // Typing it back is not a change — comparing models rather than tracking "was touched" is what
        // makes that true.
        cs.draft.name = "Development".to_string();
        assert!(!cs.is_changed());
    }

    /// A template needs a name, and an edit that empties it must not be saveable.
    #[test]
    fn a_nameless_template_cannot_be_saved() {
        let mut cs = ComponentState::new(Some(&template()));
        cs.draft.name = "   ".to_string();

        assert!(cs.is_changed());
        assert!(!cs.can_save());
    }

    /// Adding is local and immediate, and sorts by order — so the table reads as the board will.
    #[test]
    fn adding_a_column_edits_the_draft_and_sorts_it() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.new_column = NewColumn {
            id: " Review ".to_string(),
            name: " Review ".to_string(),
            description: String::new(),
            order: "5".to_string(),
        };
        cs.add_column();

        assert_eq!(
            cs.draft
                .columns
                .iter()
                .map(|itm| itm.id.as_str())
                .collect::<Vec<&str>>(),
            vec!["review", "in-progress"],
            "sorted by order, and the id trimmed and lower-cased"
        );
        assert_eq!(cs.draft.columns[0].name, "Review");
        assert!(cs.is_changed());
        assert!(cs.new_column.id.is_empty(), "the row is ready for the next");
        assert_eq!(cs.new_column.order, "5", "the order stays as the default");
    }

    /// A duplicate id is refused at the button rather than at the server: the same column twice is not a
    /// thing a board can have, and finding out after a round trip is worse.
    #[test]
    fn a_duplicate_column_id_cannot_be_added() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.new_column.id = "in-progress".to_string();
        assert!(!cs.can_add_column());

        cs.new_column.id = "review".to_string();
        assert!(cs.can_add_column());

        cs.new_column.id = "  ".to_string();
        assert!(!cs.can_add_column(), "an empty id is not addable either");
    }

    #[test]
    fn removing_a_column_is_a_change_and_restoring_it_is_not() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.remove_column("in-progress");
        assert!(cs.draft.columns.is_empty());
        assert!(cs.is_changed());

        cs.new_column = NewColumn {
            id: "in-progress".to_string(),
            name: "In progress".to_string(),
            description: String::new(),
            order: "10".to_string(),
        };
        cs.add_column();

        assert!(!cs.is_changed(), "back to exactly what arrived");
    }

    /// A new template starts empty and is not saveable until it has a name.
    #[test]
    fn a_new_template_needs_a_name_before_it_can_be_saved() {
        let mut cs = ComponentState::new(None);

        assert!(!cs.is_changed());
        assert!(!cs.can_save());

        cs.draft.name = "Development".to_string();
        assert!(cs.can_save());
    }

    /// Mid-typing must not be fought: a value that is not a number yet leaves the order alone.
    #[test]
    fn an_unparseable_order_leaves_the_column_alone() {
        let mut cs = ComponentState::new(Some(&template()));

        cs.set_column_order("in-progress", String::new());
        assert_eq!(cs.draft.columns[0].order, 10);

        cs.set_column_order("in-progress", "-".to_string());
        assert_eq!(cs.draft.columns[0].order, 10);

        cs.set_column_order("in-progress", " 25 ".to_string());
        assert_eq!(cs.draft.columns[0].order, 25);
    }
}
