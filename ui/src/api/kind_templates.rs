use flurl::{EmptyRequestModel, HttpVerb};
use task_manager_shared::kind_templates::*;

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response_opt};

/// `Ok(None)` for a non-admin: the section is admin-only and the shell asks before it knows.
pub async fn get_kind_templates() -> Result<Option<KindTemplatesResponse>, RequestError> {
    let response = authed(
        "/api/kind-templates/v1/list",
        HttpVerb::Post,
        EmptyRequestModel,
    )
    .await;

    handle_http_response_opt(response).await
}

/// Save a template whole. An empty `id` creates one.
pub async fn save_kind_template(
    id: &str,
    name: &str,
    description: &str,
    kinds: Vec<KindTemplateKind>,
) -> Result<(), RequestError> {
    let request = SaveKindTemplateInputModel {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        kinds,
    };

    handle_http_empty(authed("/api/kind-templates/v1/save", HttpVerb::Post, request).await).await
}

pub async fn delete_kind_template(id: &str) -> Result<(), RequestError> {
    let request = DeleteKindTemplateInputModel { id: id.to_string() };

    handle_http_empty(authed("/api/kind-templates/v1/delete", HttpVerb::Post, request).await).await
}
