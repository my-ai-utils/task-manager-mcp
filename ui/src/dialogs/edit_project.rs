use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::column_templates::ColumnTemplateResponse;
use task_manager_shared::kind_templates::KindTemplateResponse;
use task_manager_shared::projects::ProjectResponse;

/// What a project *is*: name, description, prefix, and which column template it follows.
///
/// Task types and members are collections with their own dialogs. Columns are not here at all: they are
/// configured once per template under Settings, and a project only picks one — which is why this dialog
/// has a dropdown where it used to have a button to a columns editor.
#[derive(Clone, PartialEq)]
struct ComponentState {
    original: Draft,
    draft: Draft,
    error: String,
    saving: bool,
}

/// The editable shape. `PartialEq` is the whole mechanism behind the Save button.
#[derive(Clone, PartialEq, Default)]
struct Draft {
    name: String,
    description: String,
    prefix: String,
    /// Empty means "follow no template" — a legitimate state whose board is Todo -> Done.
    column_template_id: String,
    /// Empty means no task types, which is legitimate too: a type is optional on a task.
    kind_template_id: String,
    /// The archive window, as typed. A STRING rather than a number because empty is a real value here —
    /// "the default" — and an `Option<i32>` parsed on every keystroke cannot tell empty from mid-typing.
    archive_days: String,
}

impl ComponentState {
    fn new(project: Option<&ProjectResponse>) -> Self {
        let draft = match project {
            Some(project) => Draft {
                name: project.name.clone(),
                description: project.description.clone(),
                prefix: project.prefix.clone(),
                column_template_id: project.column_template_id.clone().unwrap_or_default(),
                kind_template_id: project.kind_template_id.clone().unwrap_or_default(),
                archive_days: project
                    .archive_days
                    .map(|itm| itm.to_string())
                    .unwrap_or_default(),
            },
            None => Draft::default(),
        };

        Self {
            original: draft.clone(),
            draft,
            error: String::new(),
            saving: false,
        }
    }

    fn is_changed(&self) -> bool {
        self.draft != self.original
    }

    /// Name and prefix are both required — a project with no prefix could not name a single task.
    fn can_save(&self) -> bool {
        !self.saving
            && self.is_changed()
            && !self.draft.name.trim().is_empty()
            && !self.draft.prefix.trim().is_empty()
    }

    /// Whether either template changed, which decides if the extra requests are worth making at all.
    fn column_template_changed(&self) -> bool {
        self.draft.column_template_id != self.original.column_template_id
    }

    fn kind_template_changed(&self) -> bool {
        self.draft.kind_template_id != self.original.kind_template_id
    }

    fn begin_save(&mut self) {
        self.saving = true;
        self.error = String::new();
    }

    fn fail(&mut self, message: String) {
        self.saving = false;
        self.error = message;
    }
}

#[component]
pub fn EditProjectDialog(
    project: Option<Rc<ProjectResponse>>,
    on_saved: EventHandler<()>,
) -> Element {
    let mut cs = use_signal(|| ComponentState::new(project.as_deref()));
    // Its own signal rather than a field of `ComponentState`: `DataState` is neither `Clone` nor
    // `PartialEq`, and the state struct must be both — comparing it against `original` is what lights the
    // Save button. The template list is loaded data, not part of the edit, so it does not belong in the
    // comparison anyway.
    let templates_state = use_signal(DataState::<Vec<ColumnTemplateResponse>>::default);
    let kind_templates_state = use_signal(DataState::<Vec<KindTemplateResponse>>::default);
    let cs_ra = cs.read();

    let existing = project.clone();

    let submit = move |_| {
        let existing = existing.clone();
        let ra = cs.read();
        let draft = ra.draft.clone();
        let column_changed = ra.column_template_changed();
        let kind_changed = ra.kind_template_changed();
        drop(ra);

        let name = draft.name.trim().to_string();
        let description = draft.description.trim().to_string();
        let prefix = draft.prefix.trim().to_string();
        // Empty stays `None`, which is what "the default" is on the wire. Unparseable text is treated the
        // same way rather than refused: the field is numeric, so the only way to get here is a browser that
        // let something else through.
        let archive_days = draft.archive_days.trim().parse::<i32>().ok();
        let column_template_id = draft.column_template_id.clone();
        let kind_template_id = draft.kind_template_id.clone();

        cs.write().begin_save();

        spawn(async move {
            // Two calls, because the template is its own endpoint: the basics, then the assignment, and
            // only when it actually moved. A new project always needs the second one if a template was
            // picked, since create has nowhere to carry it.
            //
            // An existing project is ADDRESSED by the prefix it currently has, and may be renamed by the same
            // call — which is why what the template calls below use is the prefix out of the DRAFT and not
            // the one this dialog opened on.
            let outcome = match existing.as_deref() {
                Some(project) => {
                    crate::api::update_project(
                        &project.prefix,
                        &name,
                        &description,
                        &prefix,
                        archive_days,
                    )
                    .await
                }
                // Nothing to look up afterwards: this side chose the prefix, and the prefix is the name. It
                // used to re-read the whole project list to find the new project's internal id, purely
                // because create answers with an empty body.
                None => crate::api::create_project(&name, &description, &prefix).await,
            };

            if let Err(err) = outcome {
                cs.write().fail(err.message);
                return;
            }

            // A brand-new project needs the assignment whenever a template was picked, since create has
            // nowhere to carry it; an existing one only when the choice moved.
            let is_new = existing.is_none();
            let assign_column = is_new && !column_template_id.is_empty() || column_changed;
            let assign_kind = is_new && !kind_template_id.is_empty() || kind_changed;

            if assign_column
                && let Err(err) =
                    crate::api::set_column_template(&prefix, &column_template_id).await
            {
                // The basics DID save. Saying so matters: the person would otherwise re-enter a name that
                // is already stored and hit "prefix already used" by themselves.
                cs.write().fail(format!(
                    "The project was saved, but its column template was not: {}",
                    err.message
                ));
                on_saved.call(());
                return;
            }

            if assign_kind
                && let Err(err) =
                    crate::api::set_kind_template(&prefix, &kind_template_id).await
            {
                cs.write().fail(format!(
                    "The project was saved, but its task-type template was not: {}",
                    err.message
                ));
                on_saved.call(());
                return;
            }

            on_saved.call(());
            super::close();
        });
    };

    let title = match project.as_deref() {
        Some(project) => format!("Project {}", project.prefix),
        None => "New project".to_string(),
    };

    // Renaming a prefix keeps the old one resolving, which is worth saying on the screen where the
    // renaming happens — otherwise it looks like it would break every id already written down.
    let history = project
        .as_deref()
        .map(|project| project.prefix_history.join(", "))
        .unwrap_or_default();

    let name = cs_ra.draft.name.clone();
    let description = cs_ra.draft.description.clone();
    let prefix = cs_ra.draft.prefix.clone();
    let archive_days = cs_ra.draft.archive_days.clone();
    let selected_template = cs_ra.draft.column_template_id.clone();
    let selected_kind_template = cs_ra.draft.kind_template_id.clone();
    let error = cs_ra.error.clone();
    let can_save = cs_ra.can_save();
    let templates = read_templates(templates_state);
    let kind_templates = read_kind_templates(kind_templates_state);

    let content = rsx! {
        if !error.is_empty() {
            div { class: "error-banner", "{error}" }
        }
        div { class: "form-row",
            label { "Name" }
            input {
                r#type: "text",
                value: "{name}",
                oninput: move |event| cs.write().draft.name = event.value(),
            }
        }
        div { class: "form-row",
            label { "Task prefix" }
            input {
                r#type: "text",
                value: "{prefix}",
                placeholder: "RMS",
                oninput: move |event| cs.write().draft.prefix = event.value().to_uppercase(),
            }
            div { class: "field-hint",
                "The first half of every task id on this board — RMS-42. Two projects cannot hold the same prefix at once, and a prefix another project once used is refused too, because its old ids still resolve through it."
            }
        }
        div { class: "form-row",
            label { "Archive after" }
            input {
                r#type: "number",
                min: "1",
                value: "{archive_days}",
                placeholder: "7",
                oninput: move |event| cs.write().draft.archive_days = event.value(),
            }
            div { class: "field-hint",
                "Days a finished task stays on the board before it counts as archived — and the same window a closed goal is drawn for. Leave empty for 7. Nothing is ever deleted: archived work is still reachable by its id, and searching for one finds it."
            }
        }
        div { class: "form-row",
            label { "Description" }
            textarea {
                rows: "3",
                value: "{description}",
                oninput: move |event| cs.write().draft.description = event.value(),
            }
        }
        div { class: "form-row",
            label { "Columns" }
            // `selected` on the option, not `value` on the select — see the project picker on Home.
            select {
                onchange: move |event| cs.write().draft.column_template_id = event.value(),
                option {
                    value: "",
                    selected: selected_template.is_empty(),
                    "No template — Todo → Done only"
                }
                for template in templates.iter() {
                    option {
                        value: "{template.id}",
                        selected: template.id == selected_template,
                        "{template.name}"
                    }
                }
            }
            div { class: "field-hint",
                "Columns are configured under Settings → Column templates and shared between projects. Changing the template here moves every task parked in a column the new one does not have: it reads as Todo, and comes back if that column returns."
            }
        }
        div { class: "form-row",
            label { "Task types" }
            select {
                onchange: move |event| cs.write().draft.kind_template_id = event.value(),
                option {
                    value: "",
                    selected: selected_kind_template.is_empty(),
                    "No template — no task types"
                }
                for template in kind_templates.iter() {
                    option {
                        value: "{template.id}",
                        selected: template.id == selected_kind_template,
                        "{template.name}"
                    }
                }
            }
            div { class: "field-hint",
                "Also shared, under Settings → Task-type templates. A task pointing at a type the new template does not have reads as having none, and comes back if that type returns."
            }
        }
        if !history.is_empty() {
            div { class: "field-hint",
                "Previously: {history}. Ids written under those prefixes still resolve — the tasks_resolve_id MCP tool finds them."
            }
        }
    };

    let ok_button = rsx! {
        button { class: "btn btn-primary", disabled: !can_save, onclick: submit, "Save" }
    };

    super::dialog_template(&title, content, ok_button)
}

/// The templates to choose from, loading them on first render.
///
/// An empty list on failure rather than an error: not being able to list templates must not stop somebody
/// renaming a project, and the dropdown then simply offers only "no template".
fn read_templates(
    mut state: Signal<DataState<Vec<ColumnTemplateResponse>>>,
) -> Vec<ColumnTemplateResponse> {
    let loaded = match state.read().as_ref() {
        RenderState::Loaded(templates) => Some(templates.clone()),
        RenderState::None => None,
        _ => Some(Vec::new()),
    };

    if let Some(templates) = loaded {
        return templates;
    }

    spawn(async move {
        state.write().set_loading();

        match crate::api::get_column_templates().await {
            Ok(response) => state
                .write()
                .set_loaded(response.map(|itm| itm.templates).unwrap_or_default()),
            Err(_) => state.write().set_loaded(Vec::new()),
        }
    });

    Vec::new()
}

/// The task-type templates to choose from. Same shape and same leniency as `read_templates`.
fn read_kind_templates(
    mut state: Signal<DataState<Vec<KindTemplateResponse>>>,
) -> Vec<KindTemplateResponse> {
    let loaded = match state.read().as_ref() {
        RenderState::Loaded(templates) => Some(templates.clone()),
        RenderState::None => None,
        _ => Some(Vec::new()),
    };

    if let Some(templates) = loaded {
        return templates;
    }

    spawn(async move {
        state.write().set_loading();

        match crate::api::get_kind_templates().await {
            Ok(response) => state
                .write()
                .set_loaded(response.map(|itm| itm.templates).unwrap_or_default()),
            Err(_) => state.write().set_loaded(Vec::new()),
        }
    });

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> ProjectResponse {
        ProjectResponse {
            name: "Project".to_string(),
            description: String::new(),
            prefix: "RMS".to_string(),
            prefix_history: Vec::new(),
            columns: Vec::new(),
            column_template_id: Some("tpl".to_string()),
            column_template_name: Some("Development".to_string()),
            kind_template_id: Some("kinds".to_string()),
            kind_template_name: Some("Default".to_string()),
            kinds: Vec::new(),
            members: Vec::new(),
            tasks_amount: 0,
            archive_days: None,
            archived: false,
        }
    }

    #[test]
    fn save_is_offered_only_once_something_differs() {
        let mut cs = ComponentState::new(Some(&project()));
        assert!(!cs.can_save());

        cs.draft.name = "Renamed".to_string();
        assert!(cs.can_save());

        cs.draft.name = "Project".to_string();
        assert!(!cs.is_changed());
    }

    /// Picking another template is a change on its own, and is what decides whether the second request
    /// runs at all.
    #[test]
    fn changing_only_a_template_is_a_saveable_change() {
        let mut cs = ComponentState::new(Some(&project()));
        assert!(!cs.column_template_changed());
        assert!(!cs.kind_template_changed());

        cs.draft.column_template_id = "other".to_string();

        assert!(cs.is_changed());
        assert!(cs.can_save());
        assert!(cs.column_template_changed());
        assert!(
            !cs.kind_template_changed(),
            "the two are tracked apart, so only the request that is needed runs"
        );

        cs.draft.kind_template_id = "other-kinds".to_string();
        assert!(cs.kind_template_changed());
    }

    /// Choosing "no template" is a real choice, not a cleared field.
    #[test]
    fn clearing_a_template_is_a_change() {
        let mut cs = ComponentState::new(Some(&project()));
        cs.draft.column_template_id = String::new();

        assert!(cs.column_template_changed());
        assert!(cs.can_save());
    }

    #[test]
    fn a_project_needs_a_name_and_a_prefix() {
        let mut cs = ComponentState::new(None);
        assert!(
            !cs.can_save(),
            "an untouched new project has nothing to save"
        );

        cs.draft.name = "Project".to_string();
        assert!(!cs.can_save(), "still no prefix");

        cs.draft.prefix = "RMS".to_string();
        assert!(cs.can_save());

        cs.draft.name = "   ".to_string();
        assert!(!cs.can_save());
    }
}
