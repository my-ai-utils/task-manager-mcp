use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::auth::{GoogleCallbackInputModel, GoogleCallbackResponse};

use crate::app::AppContext;
use crate::http_server::errors::unauthorized;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/auth/v1/google-callback",
    controller: "Auth",
    summary: "Finish a Google sign-in",
    description: "Public. Exchanges the one-time code for the person's identity, checks they are allowed in, and returns a session token. Allowed in means: they are on the roster, or the settings admin list names them — in which case their row is created on the spot, which is what lets the first person into an empty database.",
    input_data: "GoogleCallbackInputModel",
    result: [
        {status_code: 200, description: "Signed in", model: "GoogleCallbackResponse"},
        {status_code: 401, description: "Google refused, or this person is not allowed in"},
    ]
)]
pub struct GoogleCallbackAction {
    app: Arc<AppContext>,
}

impl GoogleCallbackAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &GoogleCallbackAction,
    input_data: GoogleCallbackInputModel,
    _ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    // The state has to be one we issued and have not seen come back before. Without this, a third party
    // could hand a victim's browser a code of its own choosing.
    //
    // Logged rather than silently refused: this is the one step of the sign-in whose failure the person
    // in front of the screen cannot explain and neither can we, and "somebody says they cannot get in"
    // is otherwise a guess between a slow flow and a mangled parameter.
    if let Err(problem) = crate::auth::LoginState::check(&input_data.state, &action.app.session_key)
    {
        service_sdk::my_logger::LOGGER.write_warning(
            "GoogleCallback",
            format!("Refused a sign-in: {}", problem.as_log_reason()),
            service_sdk::my_logger::LogEventCtx::new(),
        );

        return Err(unauthorized(problem.as_message()));
    }

    let settings = action.app.settings_reader.get_settings().await;

    let identity = crate::auth::exchange_code(
        &settings.google_client_id,
        &settings.google_client_secret,
        &settings.google_redirect_uri,
        &input_data.code,
    )
    .await
    .map_err(|_| unauthorized("Google did not confirm the sign-in"))?;

    let admin_in_settings = action.app.is_admin_in_settings(&identity.email).await;
    let existing = action.app.board.read().get_user(&identity.email);

    match &existing {
        Some(user) if user.disabled => {
            return Err(unauthorized("This account has been disabled"));
        }
        Some(_) => {}
        // Not on the roster. Allowed only for an address the settings admin list names — that is the
        // bootstrap door, and it is the only way into a database with no users in it.
        None if admin_in_settings => {
            crate::scripts::provision_settings_admin(&action.app, &identity.email, &identity.name)
                .await;
        }
        // Logged with the address, because this is the refusal an admin has to act on: the fix is to add
        // exactly this address to the roster, and without it in a log the admin is working from whatever
        // the person managed to relay.
        None => {
            service_sdk::my_logger::LOGGER.write_warning(
                "GoogleCallback",
                format!("Refused a sign-in: {} is not on the roster", identity.email),
                service_sdk::my_logger::LogEventCtx::new(),
            );

            return Err(unauthorized("This account has not been given access"));
        }
    }

    let token = crate::auth::SessionToken::issue(identity.email).to_token(&action.app.session_key);

    // The cookie is what the browser will actually use from here on; the body carries the same token because
    // signing in is not exclusively a browser's act, and a client that wants to hold one has somewhere to read
    // it from.
    HttpOutput::from_builder()
        .set_content_as_text(
            serde_json::to_string(&GoogleCallbackResponse {
                token: token.clone(),
            })
            .unwrap_or_default(),
        )
        .set_content_type(WebContentType::Json)
        .set_cookie(crate::auth::issue_session_cookie(token))
        .into_ok_result(true)
}
