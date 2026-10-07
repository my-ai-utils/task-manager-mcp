use std::collections::BTreeSet;

use rust_extensions::AsStr;
use rust_extensions::date_time::DateTimeAsMicroseconds;
use service_sdk::my_telemetry::MyTelemetryContext;
use task_manager_shared::kind_color::KindColor;

use crate::app::AppContext;
use crate::board::{ProjectModel, is_valid_prefix};
use crate::postgres::ProjectDto;

fn validate_prefix(prefix: &str) -> Result<String, String> {
    let prefix = prefix.trim().to_uppercase();

    if !is_valid_prefix(&prefix) {
        return Err(format!(
            "'{prefix}' is not a usable prefix: use 1-16 ASCII letters, digits or underscores. A '-' is not allowed, because it separates the prefix from the number in a task id"
        ));
    }

    Ok(prefix)
}

/// A column id, validated. Lives here next to the kind version and is shared with the column-template
/// script, which applies the same rule to the same kind of hand-typed id.
pub(super) fn validate_column_id(id: &str) -> Result<String, String> {
    validate_config_id(id, "column")
}

/// A task type's id, validated. Same rule as a column's — both are hand-typed and immutable afterwards.
pub(super) fn validate_task_type_id(id: &str) -> Result<String, String> {
    validate_config_id(id, "task type")
}

/// An id typed in by a person for a column or a kind. Immutable once created, so it is worth being
/// strict here rather than living with a typo forever.
fn validate_config_id(id: &str, what: &str) -> Result<String, String> {
    let id = id.trim().to_lowercase();

    if id.is_empty() {
        return Err(format!("a {what} needs an id"));
    }

    if id.len() > 32 {
        return Err(format!("a {what} id must be 32 characters or fewer"));
    }

    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "'{id}' is not a usable {what} id: use ASCII letters, digits, '-' or '_'"
        ));
    }

    Ok(id)
}

pub(super) async fn save(app: &AppContext, project: ProjectModel) {
    let ctx = MyTelemetryContext::create_empty();
    let dto: ProjectDto = (&project).into();
    app.projects_repo.upsert(&dto, &ctx).await;

    let project_id = project.id.clone();
    app.board.upsert_project(project);
    app.notify_project_changed(&project_id).await;
}

/// Create a project. Returns its internal id — the UI works in ids, MCP in prefixes.
pub async fn create_project(
    app: &AppContext,
    name: &str,
    description: &str,
    prefix: &str,
) -> Result<String, String> {
    if name.trim().is_empty() {
        return Err("a project needs a name".to_string());
    }

    let prefix = validate_prefix(prefix)?;

    // Free means "nobody holds it right now". A prefix that only appears in some project's history is
    // takeable — which is exactly why a task's handle is composed on read rather than stored.
    if !app.board.read().is_prefix_free(&prefix, None) {
        return Err(format!(
            "prefix '{prefix}' is already used by another project"
        ));
    }

    let project = ProjectModel {
        id: rust_extensions::SortableId::generate().to_string(),
        name: name.trim().to_string(),
        description: description.trim().to_string(),
        prefix,
        prefix_history: Vec::new(),
        // No template to begin with: a new project's board is Todo -> Done until one is assigned.
        // Requiring a template up front would mean you cannot create a project before creating one.
        column_template_id: None,
        columns: Vec::new(),
        kind_template_id: None,
        kinds: Vec::new(),
        members: BTreeSet::new(),
        last_task_number: 0,
        // No window of its own: `None` is the seven-day default, which is what a project setting up its
        // board has no opinion about yet.
        archive_days: None,
        // Live. Archiving is something done to a project later, from the setup screen.
        archived_moment: None,
        // Nothing connected. A repository is attached afterwards, from the setup screen.
        github_connections: Vec::new(),
        created: DateTimeAsMicroseconds::now(),
    };

    let id = project.id.clone();
    save(app, project).await;

    Ok(id)
}

/// Rename a project, and optionally move it to another prefix.
pub async fn update_project(
    app: &AppContext,
    project_id: &str,
    name: &str,
    description: &str,
    prefix: &str,
    archive_days: Option<i32>,
) -> Result<(), String> {
    if name.trim().is_empty() {
        return Err("a project needs a name".to_string());
    }

    let prefix = validate_prefix(prefix)?;

    // Validated on the way in, where the read side is lenient: a zero or a negative would archive work the
    // instant it landed, which is not a thing anybody means to configure. `None` stays `None` — that is how
    // a project says "the default", not a value to be normalised away.
    if let Some(days) = archive_days {
        if days < 1 {
            return Err(
                "the archive window is in days and must be at least 1 — leave it empty for the default of 7"
                    .to_string(),
            );
        }

        if days > 3650 {
            return Err("an archive window longer than ten years is not a window".to_string());
        }
    }

    let board = app.board.read();
    let mut project = load(&board, project_id)?;

    if prefix != project.prefix {
        if !board.is_prefix_free(&prefix, Some(project_id)) {
            return Err(format!(
                "prefix '{prefix}' is already used by another project"
            ));
        }

        // The prefix being left behind goes into history, so `tasks_resolve_id` can still say what an
        // old handle used to mean once somebody else takes it over.
        if !project.prefix_history.contains(&project.prefix) {
            project.prefix_history.push(project.prefix.clone());
        }

        project.prefix = prefix;
    }

    project.name = name.trim().to_string();
    project.description = description.trim().to_string();
    project.archive_days = archive_days;

    save(app, project).await;
    Ok(())
}

pub(super) fn load(board: &crate::board::BoardInner, project_id: &str) -> Result<ProjectModel, String> {
    board
        .get_project(project_id)
        .map(|itm| itm.as_ref().clone())
        .ok_or_else(|| format!("no project with id '{project_id}'"))
}

/// Point a project at a column template, or at none.
///
/// The whole of a project's column configuration. There is no add-column here: columns are configured in
/// the template, and this only decides which template the project follows.
///
/// An empty id means "none" — a legitimate state whose board is Todo -> Done. A non-empty id naming a
/// template that does not exist is refused: silently accepting it would leave the project looking
/// configured while its board had nothing in the middle.
pub async fn set_column_template(
    app: &AppContext,
    project_id: &str,
    column_template_id: &str,
) -> Result<(), String> {
    let column_template_id = column_template_id.trim().to_lowercase();

    let board = app.board.read();
    let mut project = load(&board, project_id)?;

    project.column_template_id = if column_template_id.is_empty() {
        None
    } else {
        if board.get_column_template(&column_template_id).is_none() {
            return Err(format!("no column template with id '{column_template_id}'"));
        }

        Some(column_template_id)
    };

    save(app, project).await;
    Ok(())
}

/// Point a project at a task-type template, or at none.
///
/// The whole of a project's task-type configuration. There is no add-type here: types are configured in
/// the template, and this only decides which template the project follows.
pub async fn set_kind_template(
    app: &AppContext,
    project_id: &str,
    kind_template_id: &str,
) -> Result<(), String> {
    let kind_template_id = kind_template_id.trim().to_lowercase();

    let board = app.board.read();
    let mut project = load(&board, project_id)?;

    project.kind_template_id = if kind_template_id.is_empty() {
        None
    } else {
        if board.get_kind_template(&kind_template_id).is_none() {
            return Err(format!(
                "no task-type template with id '{kind_template_id}'"
            ));
        }

        Some(kind_template_id)
    };

    save(app, project).await;
    Ok(())
}

pub(super) fn parse_kind_color(color: &str) -> Result<KindColor, String> {
    color
        .trim()
        .to_lowercase()
        .parse::<KindColor>()
        .map_err(|_| {
            format!(
                "'{color}' is not one of the palette colours: {}",
                KindColor::ALL
                    .iter()
                    .map(|itm| itm.as_str())
                    .collect::<Vec<&str>>()
                    .join(", ")
            )
        })
}

/// Put a project away, or bring it back.
///
/// Its own function rather than a field on `update_project`, because it is its own act — and because
/// `update_project` is the setup form's Save, which would then archive a project as a side effect of
/// renaming it.
///
/// **Nothing else changes.** The project keeps its prefix, keeps answering `get_project_by_prefix`, keeps
/// serving its tasks and its documents, keeps following its templates, and keeps counting against those
/// templates' delete guard. Archiving takes a project out of the pickers and out of nothing else — which
/// is the whole of what a soft delete is here, and why there is no hard one.
///
/// Archiving an already archived project leaves the original moment alone: when it was put away is the
/// fact worth keeping, and pressing the same button twice is not a new one.
pub async fn set_project_archived(
    app: &AppContext,
    project_id: &str,
    archived: bool,
) -> Result<(), String> {
    let board = app.board.read();
    let mut project = load(&board, project_id)?;

    if archived {
        if project.archived_moment.is_none() {
            project.archived_moment = Some(DateTimeAsMicroseconds::now());
        }
    } else {
        project.archived_moment = None;
    }

    save(app, project).await;
    Ok(())
}

/// Replace a project's membership wholesale.
///
/// Wholesale rather than add/remove because that is how the screen works — a list of checkboxes saved
/// as a set — and because it means the caller never has to know the current state to change it.
pub async fn set_members(
    app: &AppContext,
    project_id: &str,
    emails: &[String],
) -> Result<(), String> {
    let board = app.board.read();
    let mut project = load(&board, project_id)?;

    let members: BTreeSet<String> = emails
        .iter()
        .map(|itm| itm.trim().to_lowercase())
        .filter(|itm| !itm.is_empty())
        .collect();

    let ctx = MyTelemetryContext::create_empty();
    let as_vec: Vec<String> = members.iter().cloned().collect();
    app.project_members_repo
        .replace_for_project(project_id, &as_vec, &ctx)
        .await;

    project.members = members;

    let project_id = project.id.clone();
    app.board.upsert_project(project);
    app.notify_project_changed(&project_id).await;

    Ok(())
}
