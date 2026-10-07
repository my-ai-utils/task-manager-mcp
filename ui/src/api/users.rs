use flurl::{EmptyRequestModel, HttpVerb};
use task_manager_shared::users::*;

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response_opt};

/// `Ok(None)` when the caller is not an admin — the screen says so rather than showing an error.
pub async fn get_users() -> Result<Option<UsersResponse>, RequestError> {
    let response = authed("/api/users/v1/list", HttpVerb::Post, EmptyRequestModel).await;

    handle_http_response_opt(response).await
}

pub async fn create_user(email: &str, name: &str, admin: bool) -> Result<(), RequestError> {
    let request = CreateUserInputModel {
        email: email.to_string(),
        name: name.to_string(),
        admin,
    };

    handle_http_empty(authed("/api/users/v1", HttpVerb::Post, request).await).await
}

pub async fn update_user(
    email: &str,
    name: &str,
    admin: bool,
    disabled: bool,
) -> Result<(), RequestError> {
    let request = UpdateUserInputModel {
        email: email.to_string(),
        name: name.to_string(),
        admin,
        disabled,
    };

    handle_http_empty(authed("/api/users/v1/update", HttpVerb::Post, request).await).await
}
