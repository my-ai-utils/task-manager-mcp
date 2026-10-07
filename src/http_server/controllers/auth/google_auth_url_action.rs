use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::auth::GoogleAuthUrlResponse;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/auth/v1/google-url",
    controller: "Auth",
    summary: "Where to send the browser to sign in",
    description: "Public. Returns the Google consent URL, built server-side because it carries the client id and the redirect URI from the service settings — neither of which the browser has any business knowing. Also issues the CSRF state the callback expects back.",
    result: [
        {status_code: 200, description: "The URL to redirect to", model: "GoogleAuthUrlResponse"},
        {status_code: 400, description: "Google credentials are not configured"},
    ]
)]
pub struct GoogleAuthUrlAction {
    app: Arc<AppContext>,
}

impl GoogleAuthUrlAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &GoogleAuthUrlAction,
    _ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let settings = action.app.settings_reader.get_settings().await;

    if settings.google_client_id.trim().is_empty() {
        return Err(crate::http_server::errors::bad_request(
            "Google sign-in is not configured on this deployment",
        ));
    }

    let state = crate::auth::LoginState::issue().to_token(&action.app.session_key);

    let url = crate::auth::build_auth_url(
        &settings.google_client_id,
        &settings.google_redirect_uri,
        &state,
    );

    HttpOutput::as_json(GoogleAuthUrlResponse { url }).into_ok_result(true)
}
