use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::column_templates::ColumnTemplateColumn;
use task_manager_shared::projects::is_anchor_column;

use crate::app::AppContext;
use crate::board::{ColumnModel, ColumnTemplateModel};
use crate::postgres::ColumnTemplateDto;

/// Save a template whole.
///
/// `id` empty creates; `id` naming an existing template replaces it. Either way the caller sends the
/// complete column list, so this never reconciles a delta.
///
/// Validated entirely before anything is written: a duplicate id, an anchor id or a missing name fails
/// the call and every project following the template is left exactly as it was. Half-applying a snapshot
/// would be worse than rejecting it, because the half that landed is not a state anybody asked for.
///
/// Every project following this template picks the new columns up immediately — the board resolves them
/// on the write. Tasks parked in a column this save removed keep their stored status and read as Todo;
/// putting the column back brings them home.
pub async fn save_column_template(
    app: &AppContext,
    id: &str,
    name: &str,
    description: &str,
    columns: &[ColumnTemplateColumn],
) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("a column template needs a name".to_string());
    }

    let mut next: Vec<ColumnModel> = Vec::with_capacity(columns.len());

    for column in columns {
        let column_id = super::validate_column_id(&column.id)?;

        // Todo and Done exist in every project and are added by every reader. A template carrying one
        // would draw it twice, and a task could then sit in a "todo" that is not the anchor.
        if is_anchor_column(&column_id) {
            return Err(format!(
                "'{column_id}' is one of the two columns every project already has — it cannot be in a template"
            ));
        }

        if next.iter().any(|itm| itm.id == column_id) {
            return Err(format!("column '{column_id}' is listed twice"));
        }

        if column.name.trim().is_empty() {
            return Err(format!("column '{column_id}' needs a name"));
        }

        next.push(ColumnModel {
            id: column_id,
            name: column.name.trim().to_string(),
            description: column.description.trim().to_string(),
            order: column.order,
        });
    }

    // Sorted here so every reader downstream is already in board order and none of them has to sort.
    next.sort_by_key(|itm| itm.order);

    let id = id.trim().to_lowercase();

    // Creating: keep the original `created`, so replacing a template does not make it look new.
    let (id, created) = if id.is_empty() {
        (
            rust_extensions::SortableId::generate().to_string(),
            DateTimeAsMicroseconds::now(),
        )
    } else {
        let existing = app.board.read().get_column_template(&id);

        match existing {
            Some(existing) => (id, existing.created),
            None => return Err(format!("no column template with id '{id}'")),
        }
    };

    let template = ColumnTemplateModel {
        id: id.clone(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        columns: next,
        created,
    };

    let ctx = MyTelemetryContext::create_empty();
    let dto: ColumnTemplateDto = (&template).into();
    app.column_templates_repo.upsert(&dto, &ctx).await;

    app.board.upsert_column_template(template);

    // Every board following this template just changed shape, so every one of them has to repaint.
    notify_followers(app, &id).await;

    Ok(id)
}

/// Delete a template.
///
/// Refused while any project still follows it. A project whose template vanished would lose the middle
/// of its board and every task sitting there would read as Todo — a large, silent consequence for a small
/// click. Unassigning first makes it something chosen rather than discovered.
pub async fn delete_column_template(app: &AppContext, id: &str) -> Result<(), String> {
    let id = id.trim().to_lowercase();

    let board = app.board.read();

    if board.get_column_template(&id).is_none() {
        return Err(format!("no column template with id '{id}'"));
    }

    let used_by = board.count_projects_using_template(&id);

    if used_by > 0 {
        return Err(format!(
            "{used_by} project(s) still follow this template — point them at another one first"
        ));
    }

    let ctx = MyTelemetryContext::create_empty();
    app.column_templates_repo.delete(&id, &ctx).await;

    app.board.remove_column_template(&id);

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
        .filter(|itm| itm.column_template_id.as_deref() == Some(template_id))
        .map(|itm| itm.id.clone())
        .collect();

    drop(board);

    for project_id in followers {
        app.notify_project_changed(&project_id).await;
    }
}
