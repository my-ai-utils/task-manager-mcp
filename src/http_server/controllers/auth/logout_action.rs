use std::sync::Arc;

use service_sdk::macros::use_my_http_server;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/auth/v1/logout",
    controller: "Auth",
    summary: "Sign out",
    description: "Always succeeds. It clears the session cookie, which is the whole of signing out now that the session lives in one: the token itself is not stored server-side, it is carried inside itself, so there is nothing to revoke. Revoking a live token is not possible by design — the TTL is short, and disabling a person locks them out on their very next request, which is what revocation would actually be for.",
    result: [
        {status_code: 204, description: "Signed out"},
    ]
)]
pub struct LogoutAction {
    _app: Arc<AppContext>,
}

impl LogoutAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { _app: app }
    }
}

async fn handle_request(
    _action: &LogoutAction,
    _ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // Clearing the cookie IS the sign-out: it is `HttpOnly`, so the client cannot drop it itself — which is
    // exactly the property that makes it worth having, and exactly why this endpoint now does something.
    HttpOutput::from_builder()
        .set_status_code(204)
        .set_cookie(crate::auth::clear_session_cookie())
        .into_ok_result(true)
}
