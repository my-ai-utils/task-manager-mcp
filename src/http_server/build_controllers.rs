use std::sync::Arc;

use service_sdk::HttpServerBuilder;

use crate::app::AppContext;

/// Register everything the browser talks to.
///
/// There is deliberately no task-mutating action here: every change to a task arrives through `/mcp`.
/// If you find yourself adding `register_post_action` under `tasks`, the design changed — update
/// README.md first.
pub fn build_controllers(app: &Arc<AppContext>, http_server_builder: &mut HttpServerBuilder) {
    use super::controllers::{
        auth, column_templates, documents, github, goals, kind_templates, projects, releases,
        system, tasks, templates, users,
    };

    http_server_builder.register_get_action(system::PingAction::new(app.clone()));
    http_server_builder.register_post_action(system::DiagnosticsAction::new(app.clone()));

    http_server_builder.register_post_action(auth::GoogleAuthUrlAction::new(app.clone()));
    http_server_builder.register_post_action(auth::GoogleCallbackAction::new(app.clone()));
    http_server_builder.register_post_action(auth::MeAction::new(app.clone()));
    http_server_builder.register_post_action(auth::LogoutAction::new(app.clone()));

    http_server_builder.register_post_action(projects::ListProjectsAction::new(app.clone()));
    http_server_builder.register_post_action(projects::CreateProjectAction::new(app.clone()));
    http_server_builder.register_post_action(projects::UpdateProjectAction::new(app.clone()));
    http_server_builder.register_post_action(projects::SetColumnTemplateAction::new(app.clone()));
    http_server_builder.register_post_action(projects::SetKindTemplateAction::new(app.clone()));
    http_server_builder.register_post_action(projects::SetMembersAction::new(app.clone()));
    http_server_builder.register_post_action(projects::SetArchivedAction::new(app.clone()));
    // Moving a whole board between projects. The export is a GET so it can be an ordinary download link —
    // the session is a cookie, which a navigation carries — and it streams the archive off disk rather than
    // building it in memory. The import is the same archive back the other way, as a raw body.
    http_server_builder.register_get_action(projects::ExportProjectAction::new(app.clone()));
    http_server_builder.register_post_action(projects::ImportProjectAction::new(app.clone()));

    http_server_builder
        .register_post_action(column_templates::ListTemplatesAction::new(app.clone()));
    http_server_builder
        .register_post_action(column_templates::SaveTemplateAction::new(app.clone()));
    http_server_builder
        .register_post_action(column_templates::DeleteTemplateAction::new(app.clone()));

    http_server_builder
        .register_post_action(kind_templates::ListKindTemplatesAction::new(app.clone()));
    http_server_builder
        .register_post_action(kind_templates::SaveKindTemplateAction::new(app.clone()));
    http_server_builder
        .register_post_action(kind_templates::DeleteKindTemplateAction::new(app.clone()));

    // Both kinds of template in one file, because a board needs both and they are configured together.
    // The export is a GET so it can be a download link; the import takes the YAML as a raw body, so what
    // the reader picked is what the server parses.
    http_server_builder.register_get_action(templates::ExportTemplatesAction::new(app.clone()));
    http_server_builder.register_post_action(templates::ImportTemplatesAction::new(app.clone()));

    // Reads only, like everything else the browser calls. Documents are written through /mcp, and the trash
    // is not exposed here at all — it is not a place a person browses.
    http_server_builder.register_post_action(documents::ListDocumentsAction::new(app.clone()));
    http_server_builder.register_post_action(documents::GetDocumentAction::new(app.clone()));
    // The one write on this surface — see the action for why it is allowed to exist. The archive door beside
    // it is the same write, applied once per file inside a zip.
    http_server_builder.register_post_action(documents::UploadDocumentAction::new(app.clone()));
    http_server_builder.register_post_action(documents::UploadArchiveAction::new(app.clone()));

    // Connected repositories. Setting one up is admin-only configuration; supplying a key, refreshing
    // and syncing are things a member of the project does — a mirror that empties on every deploy would
    // otherwise be one person's job to refill.
    http_server_builder.register_post_action(github::SetGithubConnectionAction::new(app.clone()));
    http_server_builder.register_post_action(github::DeleteGithubConnectionAction::new(app.clone()));
    http_server_builder.register_post_action(github::SetGithubKeyAction::new(app.clone()));
    http_server_builder.register_post_action(github::ListGithubConnectionsAction::new(app.clone()));
    http_server_builder.register_post_action(github::PullGithubConnectionAction::new(app.clone()));
    // The second write on the documents surface, and the same write as an upload: see the action.
    http_server_builder.register_post_action(github::SyncGithubAction::new(app.clone()));

    http_server_builder.register_post_action(goals::ListGoalsAction::new(app.clone()));
    http_server_builder.register_post_action(goals::SetGoalColorAction::new(app.clone()));
    // A read, like the two lists around it. A release is recorded through /mcp.
    http_server_builder.register_post_action(releases::ListReleasesAction::new(app.clone()));
    // One of them by id — what a release's own page, the thing a shared link opens, is drawn from.
    http_server_builder.register_post_action(releases::GetReleaseAction::new(app.clone()));
    http_server_builder.register_post_action(tasks::ListTasksAction::new(app.clone()));
    http_server_builder.register_post_action(tasks::MoveTaskAction::new(app.clone()));
    http_server_builder.register_post_action(tasks::FindTaskAction::new(app.clone()));

    http_server_builder.register_post_action(users::ListUsersAction::new(app.clone()));
    http_server_builder.register_post_action(users::CreateUserAction::new(app.clone()));
    http_server_builder.register_post_action(users::UpdateUserAction::new(app.clone()));
}
