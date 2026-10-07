use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::project_transfer::{
    ImportProjectInputModel, ImportProjectResponse, SkippedImportEntryResponse,
};

use crate::app::AppContext;
use crate::http_server::errors::{bad_request, not_found};

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/projects/v1/import",
    controller: "Projects",
    summary: "Pour an exported project into this one",
    description: "The other half of /api/projects/v1/export: the zip goes up as the raw body, and everything in it is added to the project named in the query. ADDITIVE — nothing already on this board is touched or removed; goals, tasks and releases arrive alongside what is there, taking fresh numbers out of this project's own counter, and every reference inside the file is remapped to them: a task's goal, its dependencies, the releases a goal lists, and the target of every comment. Comments keep their own author and moment rather than being re-signed by whoever pressed Import. A document lands at its path, and one already at that path gets a NEW VERSION of itself, keeping its id and its whole history. THE PROJECT'S SETTINGS ARE REPLACED with the file's — name, description, archive window and the two template ids — because the statuses in the file are the source board's column ids and mean nothing unless this project follows the same template; a template that is not on this instance, or a prefix another project holds, is left alone and reported in `notes`. PARTIAL BY DESIGN: an entry that cannot be written comes back in `skipped` with a reason and the rest still arrives. Admin only.",
    input_data: "ImportProjectInputModel",
    result: [
        {status_code: 200, description: "What landed, what did not, and what reads differently here", model: "ImportProjectResponse"},
        {status_code: 400, description: "Not a project export, or more than one import may carry"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
        {status_code: 404, description: "No such project"},
    ]
)]
pub struct ImportProjectAction {
    app: Arc<AppContext>,
}

impl ImportProjectAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ImportProjectAction,
    input_data: ImportProjectInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Admin, unlike the export beside it. Reading a board is a member's business; writing several hundred
    // cards into one — and replacing its settings while doing so — is configuration, and it sits on the
    // admin-only setup screen with the rest of it.
    //
    // The identity is also what signs every document version this writes, which is the one thing in an
    // import that is honestly attributable to the person who pressed the button: a document arriving here
    // IS a new version written now, where a comment is a record of something said elsewhere.
    let user = crate::auth::require_admin(&action.app, ctx).await?;

    let project_prefix = {
        let board = action.app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, &input_data.project) {
            Ok(project) => project.prefix.clone(),
            Err(err) => return Err(not_found(err)),
        }
    };

    let outcome = crate::scripts::import_project(
        &action.app,
        &project_prefix,
        &input_data.content,
        &user.email,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::as_json(ImportProjectResponse {
        goals: outcome.goals as i32,
        tasks: outcome.tasks as i32,
        comments: outcome.comments as i32,
        documents: outcome.documents as i32,
        releases: outcome.releases as i32,
        skipped: outcome
            .skipped
            .into_iter()
            .map(|itm| SkippedImportEntryResponse {
                name: itm.name,
                reason: itm.reason,
            })
            .collect(),
        notes: outcome.notes,
    })
    .into_ok_result(true)
}
