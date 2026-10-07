use flurl::{EmptyRequestModel, HttpVerb};
use task_manager_shared::column_templates::*;

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response_opt};

/// `Ok(None)` for a non-admin: the section is admin-only and the shell asks before it knows.
pub async fn get_column_templates() -> Result<Option<ColumnTemplatesResponse>, RequestError> {
    let response = authed(
        "/api/column-templates/v1/list",
        HttpVerb::Post,
        EmptyRequestModel,
    )
    .await;

    handle_http_response_opt(response).await
}

/// Save a template whole. An empty `id` creates one.
pub async fn save_column_template(
    id: &str,
    name: &str,
    description: &str,
    columns: Vec<ColumnTemplateColumn>,
) -> Result<(), RequestError> {
    let request = SaveColumnTemplateInputModel {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        columns,
    };

    handle_http_empty(authed("/api/column-templates/v1/save", HttpVerb::Post, request).await).await
}

pub async fn delete_column_template(id: &str) -> Result<(), RequestError> {
    let request = DeleteColumnTemplateInputModel { id: id.to_string() };

    handle_http_empty(authed("/api/column-templates/v1/delete", HttpVerb::Post, request).await)
        .await
}
