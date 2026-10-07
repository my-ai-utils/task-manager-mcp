use flurl::{EmptyRequestModel, FlUrl, HttpVerb};
use task_manager_shared::auth::*;

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response, handle_http_response_opt};

/// Where to send the browser to sign in. Built server-side because it carries the client id and the
/// redirect URI, neither of which this side has any business knowing.
pub async fn get_google_auth_url() -> Result<GoogleAuthUrlResponse, RequestError> {
    let response = FlUrl::new("/api/auth/v1/google-url")
        .execute_request(HttpVerb::Post, EmptyRequestModel)
        .await;

    handle_http_response(response).await
}

pub async fn finish_google_login(
    code: &str,
    state: &str,
) -> Result<GoogleCallbackResponse, RequestError> {
    let request = GoogleCallbackInputModel {
        code: code.to_string(),
        state: state.to_string(),
    };

    // POST with a body: a `+` in a query value reads as a space, and both of these regularly contain one.
    let response = FlUrl::new("/api/auth/v1/google-callback")
        .execute_request(HttpVerb::Post, request)
        .await;

    handle_http_response(response).await
}

/// `Ok(None)` means no session — the shell then shows the login screen instead of an error.
pub async fn get_me() -> Result<Option<MeResponse>, RequestError> {
    let response = authed("/api/auth/v1/me", HttpVerb::Post, EmptyRequestModel).await;

    handle_http_response_opt(response).await
}

pub async fn logout() -> Result<(), RequestError> {
    let response = authed("/api/auth/v1/logout", HttpVerb::Post, EmptyRequestModel).await;

    handle_http_empty(response).await
}
