use std::sync::Arc;

use service_sdk::macros::use_my_http_server;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "GET",
    route: "/api/templates/v1/export",
    controller: "Templates",
    summary: "Download every column and task-type template as one YAML file",
    description: "The configuration a board is built from — the named sets of columns and of task types — as a single readable document. A plain file rather than an archive, because templates are a handful of ids, names and colours: there is nothing to stream and nothing to put in a folder, and one YAML is the artefact somebody actually wants — reviewable in a diff, editable by hand, keepable in a repository. Prose (names, descriptions) travels base64 inside it so it cannot be re-interpreted or re-indented; ids, colours, icons and orders stay legible. Both lists are sorted by id, so two exports of an unchanged instance are byte-identical. THIS IS THE OTHER HALF OF MOVING A PROJECT: a project export carries the ID of the templates it follows, not the templates themselves, so bringing these over first is what stops the imported board arriving pointed at configuration that is not there. A GET so it can be an ordinary download link — the session is a cookie, which the browser attaches to a navigation.",
    result: [
        {status_code: 200, description: "The templates file"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct ExportTemplatesAction {
    app: Arc<AppContext>,
}

impl ExportTemplatesAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ExportTemplatesAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Admin, like everything else under Settings: templates are instance-wide configuration, and this
    // hands back the whole of it.
    crate::auth::require_admin(&action.app, ctx).await?;

    // Served from memory and small enough to hand over whole — unlike a project export, which is built
    // onto disk and streamed because its documents set no bound on its size.
    let exported = crate::scripts::export_templates(&action.app);

    HttpOutput::as_file(exported.file_name, exported.content).into_ok_result(false)
}
