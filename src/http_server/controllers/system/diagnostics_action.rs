use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::system::DiagnosticsResponse;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/system/v1/diagnostics",
    controller: "System",
    summary: "What this deployment is configured with",
    description: "Admin only, read-only. There is nothing to configure here — the Google credentials and the admin list come from the service settings, not the database. It exists because the first question when a sign-in fails is which client id was picked up and which redirect URI is expected, and guessing that from logs is worse than reading it.",
    result: [
        {status_code: 200, description: "The configuration in effect", model: "DiagnosticsResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct DiagnosticsAction {
    app: Arc<AppContext>,
}

impl DiagnosticsAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &DiagnosticsAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    let settings = action.app.settings_reader.get_settings().await;
    let board = action.app.board.read();

    // The client id is not a secret and is worth showing in full — a truncated one cannot be compared
    // against the Google console, which is the entire point of the screen. The secret is never returned.
    HttpOutput::as_json(DiagnosticsResponse {
        google_configured: !settings.google_client_id.trim().is_empty()
            && !settings.google_client_secret.trim().is_empty(),
        google_client_id: settings.google_client_id.clone(),
        google_redirect_uri: settings.google_redirect_uri.clone(),
        admins_in_settings: settings.admins.len() as i32,
        projects_amount: board.projects().len() as i32,
        users_amount: board.users().len() as i32,
        connected_homes: action.app.subscribers.amount() as i32,
    })
    .into_ok_result(true)
}
