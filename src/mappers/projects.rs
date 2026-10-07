use std::collections::BTreeSet;

use rust_extensions::AsStr;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::projects::{ProjectColumnResponse, ProjectKindResponse, ProjectResponse};

use crate::board::{
    ColumnModel, ColumnTemplateModel, GithubConnectionModel, KindModel, KindTemplateModel,
    ProjectModel,
};
use crate::postgres::{
    ColumnTemplateColumnJsonModel, ColumnTemplateDto, KindTemplateDto, KindTemplateKindJsonModel,
    ProjectColumnJsonModel, ProjectDto, ProjectGithubConnectionJsonModel, ProjectKindJsonModel,
};

// Conversions are `From` rather than `Into`. The house style says "always an impl, never a standalone
// `map_x_to_y` function", and `From` satisfies that while also giving `Into` for free — and clippy's
// `from_over_into` rejects the other direction, which matters under `-D warnings`.

impl From<&ProjectColumnJsonModel> for ColumnModel {
    fn from(src: &ProjectColumnJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            order: src.column_order,
        }
    }
}

impl From<&ColumnModel> for ProjectColumnJsonModel {
    fn from(src: &ColumnModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            column_order: src.order,
        }
    }
}

impl From<&ProjectKindJsonModel> for KindModel {
    fn from(src: &ProjectKindJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            // A colour this build does not recognise is decoration that failed to load, not a
            // corrupt row — it draws as the default swatch.
            color: KindColor::parse_or_default(&src.color),
            // The dead per-project shape predates icons and never carried one.
            icon: String::new(),
        }
    }
}

impl From<&KindModel> for ProjectKindJsonModel {
    fn from(src: &KindModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            color: src.color.as_str().to_string(),
        }
    }
}

impl From<&ProjectGithubConnectionJsonModel> for GithubConnectionModel {
    fn from(src: &ProjectGithubConnectionJsonModel) -> Self {
        Self {
            name: src.name.clone(),
            owner: src.owner.clone(),
            repo: src.repo.clone(),
            branch: src.branch.clone(),
            repo_path: src.repo_path.clone(),
        }
    }
}

impl From<&GithubConnectionModel> for ProjectGithubConnectionJsonModel {
    fn from(src: &GithubConnectionModel) -> Self {
        Self {
            name: src.name.clone(),
            owner: src.owner.clone(),
            repo: src.repo.clone(),
            branch: src.branch.clone(),
            repo_path: src.repo_path.clone(),
        }
    }
}

/// Postgres row -> memory. Membership is not in the project row (it has its own table), so it starts
/// empty and the loader fills it.
impl From<&ProjectDto> for ProjectModel {
    fn from(src: &ProjectDto) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            prefix: src.prefix.to_uppercase(),
            prefix_history: src
                .prefix_history
                .iter()
                .map(|itm| itm.to_uppercase())
                .collect(),
            column_template_id: src.column_template_id.clone(),
            // Left empty on purpose: `BoardInner::rebuild_indexes` fills both from their templates.
            // Reading the row's own (dead) columns here would resurrect a value nothing maintains.
            columns: Vec::new(),
            kind_template_id: src.kind_template_id.clone(),
            kinds: Vec::new(),
            members: BTreeSet::new(),
            last_task_number: src.last_task_number,
            archive_days: src.archive_days,
            archived_moment: src.archived_moment,
            // NULL is a project that has never connected a repository, which is every row written before
            // the column existed.
            github_connections: src
                .github_connections
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|itm| itm.into())
                .collect(),
            created: src.created,
        }
    }
}

/// Memory -> Postgres row. Membership is deliberately absent: it lives in `project_members`, and a
/// project row that also carried it would give the same fact two homes.
impl From<&ProjectModel> for ProjectDto {
    fn from(src: &ProjectModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            prefix: src.prefix.clone(),
            prefix_history: src.prefix_history.clone(),
            column_template_id: src.column_template_id.clone(),
            kind_template_id: src.kind_template_id.clone(),
            // Both dead. Written empty — what they used to hold lives in the templates now.
            columns: Vec::new(),
            kinds: Vec::new(),
            last_task_number: src.last_task_number,
            archive_days: src.archive_days,
            archived_moment: src.archived_moment,
            // Always written as a list, never as NULL: NULL is what an older build left behind, not
            // something this one produces. An empty list and NULL read the same way coming back.
            github_connections: Some(
                src.github_connections
                    .iter()
                    .map(|itm| itm.into())
                    .collect(),
            ),
            created: src.created,
        }
    }
}

/// Memory -> wire.
///
/// The two anchor columns are not part of `columns` — a client adds Todo at the start and Done at the
/// end.
///
/// `tasks_amount` is derived against the board rather than read off the project, so it is passed in.
/// There is no `labels` here on purpose: the UI configures projects and never tags anything, so the
/// label vocabulary is an MCP-only concern and lives on the MCP view instead.
pub fn project_to_response(
    src: &ProjectModel,
    tasks_amount: usize,
    column_template: Option<&ColumnTemplateModel>,
    kind_template: Option<&KindTemplateModel>,
) -> ProjectResponse {
    ProjectResponse {
        name: src.name.clone(),
        description: src.description.clone(),
        prefix: src.prefix.clone(),
        prefix_history: src.prefix_history.clone(),
        columns: src
            .columns
            .iter()
            .map(|itm| ProjectColumnResponse {
                id: itm.id.clone(),
                name: itm.name.clone(),
                description: itm.description.clone(),
                order: itm.order,
            })
            .collect(),
        kinds: src
            .kinds
            .iter()
            .map(|itm| ProjectKindResponse {
                id: itm.id.clone(),
                name: itm.name.clone(),
                description: itm.description.clone(),
                color: itm.color.as_str().to_string(),
                icon: itm.icon.clone(),
            })
            .collect(),
        members: src.members.iter().cloned().collect(),
        tasks_amount: tasks_amount as i32,
        archive_days: src.archive_days,
        // The flag travels even though one of this endpoint's two readers must hide it. `/api/projects/v1/list`
        // feeds both the pickers, which leave archived projects out, and the setup table, which shows them
        // under a toggle — so the server sends everything and each reader decides. Filtering here would mean
        // a second endpoint or a refetch on every click of that toggle.
        archived: src.is_archived(),
        column_template_id: src.column_template_id.clone(),
        // The NAME as well as the id, so the setup screen can say which template a project follows
        // without holding the whole template list to look it up. `None` covers both "follows none" and
        // "follows one that is gone" — which read the same way on a board, so they read the same here.
        column_template_name: column_template.map(|itm| itm.name.clone()),
        kind_template_id: src.kind_template_id.clone(),
        kind_template_name: kind_template.map(|itm| itm.name.clone()),
    }
}

impl From<&KindTemplateKindJsonModel> for KindModel {
    fn from(src: &KindTemplateKindJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            // A colour this build does not recognise is decoration that failed to load, not a corrupt
            // row — it draws as the default swatch.
            color: KindColor::parse_or_default(&src.color),
            icon: src.icon.clone(),
        }
    }
}

impl From<&KindModel> for KindTemplateKindJsonModel {
    fn from(src: &KindModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            color: src.color.as_str().to_string(),
            icon: src.icon.clone(),
        }
    }
}

impl From<&KindTemplateDto> for KindTemplateModel {
    fn from(src: &KindTemplateDto) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            kinds: src.kinds.iter().map(|itm| itm.into()).collect(),
            created: src.created,
        }
    }
}

impl From<&KindTemplateModel> for KindTemplateDto {
    fn from(src: &KindTemplateModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            kinds: src.kinds.iter().map(|itm| itm.into()).collect(),
            created: src.created,
        }
    }
}

/// Memory -> wire, with the count of projects following it passed in — it is derived against the whole
/// board, which a template does not know about.
pub fn kind_template_to_response(
    src: &KindTemplateModel,
    used_by: usize,
) -> task_manager_shared::kind_templates::KindTemplateResponse {
    task_manager_shared::kind_templates::KindTemplateResponse {
        id: src.id.clone(),
        name: src.name.clone(),
        description: src.description.clone(),
        kinds: src
            .kinds
            .iter()
            .map(
                |itm| task_manager_shared::kind_templates::KindTemplateKind {
                    id: itm.id.clone(),
                    name: itm.name.clone(),
                    description: itm.description.clone(),
                    color: itm.color.as_str().to_string(),
                    icon: itm.icon.clone(),
                },
            )
            .collect(),
        used_by: used_by as i32,
    }
}

impl From<&ColumnTemplateColumnJsonModel> for ColumnModel {
    fn from(src: &ColumnTemplateColumnJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            order: src.column_order,
        }
    }
}

impl From<&ColumnModel> for ColumnTemplateColumnJsonModel {
    fn from(src: &ColumnModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            column_order: src.order,
        }
    }
}

impl From<&ColumnTemplateDto> for ColumnTemplateModel {
    fn from(src: &ColumnTemplateDto) -> Self {
        let mut columns: Vec<ColumnModel> = src.columns.iter().map(|itm| itm.into()).collect();
        // Sorted once here so every reader is already in board order.
        columns.sort_by_key(|itm| itm.order);

        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            columns,
            created: src.created,
        }
    }
}

impl From<&ColumnTemplateModel> for ColumnTemplateDto {
    fn from(src: &ColumnTemplateModel) -> Self {
        Self {
            id: src.id.clone(),
            name: src.name.clone(),
            description: src.description.clone(),
            columns: src.columns.iter().map(|itm| itm.into()).collect(),
            created: src.created,
        }
    }
}

/// Memory -> wire, with the count of projects following it passed in — it is derived against the whole
/// board, which a template does not know about.
pub fn column_template_to_response(
    src: &ColumnTemplateModel,
    used_by: usize,
) -> task_manager_shared::column_templates::ColumnTemplateResponse {
    task_manager_shared::column_templates::ColumnTemplateResponse {
        id: src.id.clone(),
        name: src.name.clone(),
        description: src.description.clone(),
        columns: src
            .columns
            .iter()
            .map(
                |itm| task_manager_shared::column_templates::ColumnTemplateColumn {
                    id: itm.id.clone(),
                    name: itm.name.clone(),
                    description: itm.description.clone(),
                    order: itm.order,
                },
            )
            .collect(),
        used_by: used_by as i32,
    }
}
