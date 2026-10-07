use std::collections::BTreeSet;

use ahash::AHashMap;
use service_sdk::my_telemetry::MyTelemetryContext;

use crate::app::AppContext;
use crate::board::{
    BoardInner, ColumnTemplateModel, GoalModel, KindTemplateModel, ProjectModel, ReleaseModel,
    TaskModel, UserModel,
};

/// Read the whole product out of Postgres and install it in memory.
///
/// Called once, before the HTTP server starts serving. Everything after this point reads from memory;
/// Postgres is only ever written again. If it fails, it panics through the repos' `.expect` — which is
/// correct: a service that cannot read its own state has nothing useful to serve, and the SDK turns
/// the panic into a FatalError rather than leaving an empty board looking like a fresh install.
pub async fn load_state(app: &AppContext) {
    let ctx = MyTelemetryContext::create_empty();

    let project_rows = app.projects_repo.get_all(&ctx).await;
    let member_rows = app.project_members_repo.get_all(&ctx).await;
    let task_rows = app.tasks_repo.get_all(&ctx).await;
    let user_rows = app.users_repo.get_all(&ctx).await;
    let template_rows = app.column_templates_repo.get_all(&ctx).await;
    let kind_template_rows = app.kind_templates_repo.get_all(&ctx).await;
    let goal_rows = app.goals_repo.get_all(&ctx).await;
    let release_rows = app.releases_repo.get_all(&ctx).await;

    // Membership lives in its own table, so it is folded back onto the projects here — the only place
    // the two halves are joined.
    let mut members_by_project: AHashMap<String, BTreeSet<String>> = AHashMap::new();
    for row in &member_rows {
        members_by_project
            .entry(row.project_id.clone())
            .or_default()
            .insert(row.email.trim().to_lowercase());
    }

    let projects: Vec<ProjectModel> = project_rows
        .iter()
        .map(|row| {
            let mut project: ProjectModel = row.into();

            if let Some(members) = members_by_project.remove(&project.id) {
                project.members = members;
            }

            project
        })
        .collect();

    let tasks: Vec<TaskModel> = task_rows.iter().map(|row| row.into()).collect();
    let users: Vec<UserModel> = user_rows.iter().map(|row| row.into()).collect();

    let column_templates: Vec<ColumnTemplateModel> =
        template_rows.iter().map(|row| row.into()).collect();

    let kind_templates: Vec<KindTemplateModel> =
        kind_template_rows.iter().map(|row| row.into()).collect();

    // Before the index is built from them: the backfills write columns onto rows that predate them, and an
    // index built first would carry the gaps until the next restart — which for a hash means every legacy
    // document reading as "never briefed" for the life of the process.
    crate::scripts::backfill_document_columns(app).await;
    crate::scripts::backfill_content_hashes(app).await;

    let document_rows = app.documents_repo.get_all_indexed(&ctx).await;

    // Every brief there is, in one query: they are keyed by content rather than by project, and a listing
    // joins them by hash out of memory. See `crate::documents::BriefsIndex`.
    app.briefs
        .replace_all(&app.documents_repo.get_all_briefs(&ctx).await);

    // Its own collection, installed beside the board rather than inside it — the board is pushed whole down a
    // socket and documents must not ride along.
    app.documents_index.replace_all(&document_rows);

    app.board.replace_all(BoardInner::from_loaded(
        projects,
        tasks,
        users,
        column_templates,
        kind_templates,
        goal_rows
            .iter()
            .map(|row| row.into())
            .collect::<Vec<GoalModel>>(),
        release_rows
            .iter()
            .map(|row| row.into())
            .collect::<Vec<ReleaseModel>>(),
    ));
}
