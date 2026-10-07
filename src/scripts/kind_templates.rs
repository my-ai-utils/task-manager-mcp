use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::kind_templates::KindTemplateKind;

use crate::app::AppContext;
use crate::board::{KindModel, KindTemplateModel};
use crate::postgres::KindTemplateDto;

/// Save a template whole.
///
/// `id` empty creates; `id` naming an existing template replaces it. Either way the caller sends the
/// complete list, so this never reconciles a delta.
///
/// Validated entirely before anything is written: a duplicate id, a missing name or an unknown colour
/// fails the call and every project following the template is left exactly as it was. Half-applying a
/// snapshot would be worse than rejecting it, because the half that landed is not a state anybody asked
/// for.
///
/// Every project following this template picks the new types up immediately. A task pointing at a type
/// this save removed keeps its stored value and reads as having no type; putting the type back brings it
/// home.
pub async fn save_kind_template(
    app: &AppContext,
    id: &str,
    name: &str,
    description: &str,
    kinds: &[KindTemplateKind],
) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("a task-type template needs a name".to_string());
    }

    let mut next: Vec<KindModel> = Vec::with_capacity(kinds.len());

    for kind in kinds {
        let kind_id = super::validate_task_type_id(&kind.id)?;

        if next.iter().any(|itm| itm.id == kind_id) {
            return Err(format!("task type '{kind_id}' is listed twice"));
        }

        if kind.name.trim().is_empty() {
            return Err(format!("task type '{kind_id}' needs a name"));
        }

        next.push(KindModel {
            id: kind_id,
            name: kind.name.trim().to_string(),
            description: kind.description.trim().to_string(),
            color: super::parse_kind_color(&kind.color)?,
            icon: kind.icon.trim().to_string(),
        });
    }

    let id = id.trim().to_lowercase();

    // Creating: keep the original `created`, so replacing a template does not make it look new.
    let (id, created) = if id.is_empty() {
        (
            rust_extensions::SortableId::generate().to_string(),
            DateTimeAsMicroseconds::now(),
        )
    } else {
        let existing = app.board.read().get_kind_template(&id);

        match existing {
            Some(existing) => (id, existing.created),
            None => return Err(format!("no column template with id '{id}'")),
        }
    };

    let template = KindTemplateModel {
        id: id.clone(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        kinds: next,
        created,
    };

    let ctx = MyTelemetryContext::create_empty();
    let dto: KindTemplateDto = (&template).into();
    app.kind_templates_repo.upsert(&dto, &ctx).await;

    app.board.upsert_kind_template(template);

    // Every board following this template just changed how its stickers are labelled, so every one of
    // them has to repaint.
    notify_followers(app, &id).await;

    Ok(id)
}

/// Delete a template.
///
/// Refused while any project still follows it. Losing a task type is not destructive the way losing a
/// column is — a task pointing at one that is gone just reads as having no type — but it still changes
/// every following project at once, so it is made deliberately rather than discovered.
pub async fn delete_kind_template(app: &AppContext, id: &str) -> Result<(), String> {
    let id = id.trim().to_lowercase();

    let board = app.board.read();

    if board.get_kind_template(&id).is_none() {
        return Err(format!("no column template with id '{id}'"));
    }

    let used_by = board.count_projects_using_template(&id);

    if used_by > 0 {
        return Err(format!(
            "{used_by} project(s) still follow this template — point them at another one first"
        ));
    }

    let ctx = MyTelemetryContext::create_empty();
    app.kind_templates_repo.delete(&id, &ctx).await;

    app.board.remove_kind_template(&id);

    Ok(())
}

/// Tell every project following `template_id` that its board changed.
///
/// Read AFTER the write, so it covers exactly the projects that follow the template now.
async fn notify_followers(app: &AppContext, template_id: &str) {
    let board = app.board.read();

    let followers: Vec<String> = board
        .projects()
        .iter()
        .filter(|itm| itm.kind_template_id.as_deref() == Some(template_id))
        .map(|itm| itm.id.clone())
        .collect();

    drop(board);

    for project_id in followers {
        app.notify_project_changed(&project_id).await;
    }
}
