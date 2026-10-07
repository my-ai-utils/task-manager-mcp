mod auth;
mod column_templates;
mod documents;
mod github;
mod goals;
mod kind_templates;
mod projects;
mod releases;
mod system;
mod tasks;
mod templates_transfer;
mod users;

pub use auth::*;
pub use column_templates::*;
pub use documents::*;
pub use github::*;
pub use goals::*;
pub use kind_templates::*;
pub use projects::*;
pub use releases::*;
pub use system::*;
pub use tasks::*;
pub use templates_transfer::*;
pub use users::*;

use flurl::{FlUrl, FlUrlError, FlUrlResponse, HttpVerb};
use my_http_utils::schema::client::THttpRequestBuilder;
use serde::de::DeserializeOwned;

use crate::models::RequestError;

fn is_success(status: u16) -> bool {
    (200..300).contains(&status)
}

/// The server writes its business errors as prose for whoever caused them, so the body *is* the message.
async fn read_error_body(response: &mut FlUrlResponse) -> RequestError {
    let message = response
        .get_body_as_str()
        .await
        .map(|body| body.to_string())
        .unwrap_or_else(|err| err.to_string());

    RequestError { message }
}

/// 2xx → deserialize; anything else → the response body as the error.
pub async fn handle_http_response<T: DeserializeOwned>(
    response: Result<FlUrlResponse, FlUrlError>,
) -> Result<T, RequestError> {
    let mut response = response?;

    if is_success(response.get_status_code()) {
        return Ok(response.get_json().await?);
    }

    Err(read_error_body(&mut response).await)
}

/// For endpoints with no response body.
pub async fn handle_http_empty(
    response: Result<FlUrlResponse, FlUrlError>,
) -> Result<(), RequestError> {
    let mut response = response?;

    if is_success(response.get_status_code()) {
        return Ok(());
    }

    Err(read_error_body(&mut response).await)
}

/// Like [`handle_http_response`], but 401 and 403 come back as `Ok(None)`.
///
/// Used where "not signed in" or "not an admin" is an expected answer rather than a failure — `/me` on
/// first load, and the admin-only screens, which the shell needs to be able to ask about without
/// treating the refusal as an error to show.
pub async fn handle_http_response_opt<T: DeserializeOwned>(
    response: Result<FlUrlResponse, FlUrlError>,
) -> Result<Option<T>, RequestError> {
    let mut response = response?;

    let status = response.get_status_code();

    if status == 401 || status == 403 {
        return Ok(None);
    }

    if is_success(status) {
        return Ok(Some(response.get_json().await?));
    }

    Err(read_error_body(&mut response).await)
}

/// A request carrying the session, for any verb.
///
/// **It attaches nothing, and that is the point.** The session is an `HttpOnly` cookie now, so the browser
/// sends it — `fetch` defaults to `credentials: "same-origin"` and every url here is relative, which is what
/// makes them same-origin. The client therefore cannot read the token, cannot leak it into a url, and cannot
/// hand it to a page it renders.
///
/// The function stays rather than being inlined: every call site reads as "this one needs a session", and the
/// day something has to be attached again there is one place to do it.
///
/// URLs are relative: FlUrl's wasm backend resolves `/api/...` against the page origin, so no base URL is ever
/// computed or configured.
async fn authed<TModel: THttpRequestBuilder>(
    url: &str,
    verb: HttpVerb,
    model: TModel,
) -> Result<FlUrlResponse, FlUrlError> {
    FlUrl::new(url).execute_request(verb, model).await
}
