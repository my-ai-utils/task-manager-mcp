use flurl::{EmptyRequestModel, HttpVerb};
use task_manager_shared::project_transfer::{ImportProjectInputModel, ImportProjectResponse};
use task_manager_shared::projects::*;

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response};

// Every mutation here is a POST to a STATIC url, with the ids in the body. Not a style choice — a
// constraint of the request builder: `#[http_path]` fields are APPENDED to the url in declaration order
// (`__url.append_path_segment(...)`), there is no `{name}` substitution. So a route may not carry a
// static segment after a parameter. `/api/projects/v1/{projectId}/columns` cannot be addressed at all:
// the client can only produce `/api/projects/v1/{projectId}`, which is a 404, and that is exactly the
// bug this shape replaced. Param-last (`/api/users/v1/{email}`) is fine and is used where it fits.

pub async fn get_projects() -> Result<ProjectsResponse, RequestError> {
    let response = authed("/api/projects/v1/list", HttpVerb::Post, EmptyRequestModel).await;

    handle_http_response(response).await
}

pub async fn create_project(
    name: &str,
    description: &str,
    prefix: &str,
) -> Result<(), RequestError> {
    let request = CreateProjectInputModel {
        name: name.to_string(),
        description: description.to_string(),
        prefix: prefix.to_string(),
    };

    handle_http_empty(authed("/api/projects/v1", HttpVerb::Post, request).await).await
}

pub async fn update_project(
    project: &str,
    name: &str,
    description: &str,
    prefix: &str,
    archive_days: Option<i32>,
) -> Result<(), RequestError> {
    let request = UpdateProjectInputModel {
        project: project.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        prefix: prefix.to_string(),
        archive_days,
    };

    handle_http_empty(authed("/api/projects/v1/update", HttpVerb::Post, request).await).await
}

/// Point a project at a column template, or at none with an empty id.
pub async fn set_column_template(
    project: &str,
    column_template_id: &str,
) -> Result<(), RequestError> {
    let request = SetProjectColumnTemplateInputModel {
        project: project.to_string(),
        column_template_id: column_template_id.to_string(),
    };

    handle_http_empty(
        authed(
            "/api/projects/v1/column-template/set",
            HttpVerb::Post,
            request,
        )
        .await,
    )
    .await
}

/// Point a project at a task-type template, or at none with an empty id.
pub async fn set_kind_template(
    project: &str,
    kind_template_id: &str,
) -> Result<(), RequestError> {
    let request = SetProjectKindTemplateInputModel {
        project: project.to_string(),
        kind_template_id: kind_template_id.to_string(),
    };

    handle_http_empty(
        authed(
            "/api/projects/v1/kind-template/set",
            HttpVerb::Post,
            request,
        )
        .await,
    )
    .await
}

/// Pour an exported project into this one.
///
/// **The one call on this client that sends a file as itself.** A document upload base64s its bytes into a
/// JSON body, which is right for one file somebody picked; this is a whole board — every task, every comment
/// and every document — and base64 would cost a third more on the wire and hold the encoded copy beside the
/// decoded one in a wasm heap. The shared model carries the zip as `#[http_body_raw]`, so the request builder
/// puts the bytes in the body verbatim and the project rides in the query string.
///
/// There is no export counterpart here, and there is not meant to be: a download is a navigation, not a
/// `fetch` — see `export_project_url` in the shared crate.
pub async fn import_project(
    project: &str,
    bytes: Vec<u8>,
) -> Result<ImportProjectResponse, RequestError> {
    let request = ImportProjectInputModel {
        project: project.to_string(),
        content: bytes,
    };

    handle_http_response(authed("/api/projects/v1/import", HttpVerb::Post, request).await).await
}

/// Put a project away, or bring it back. Admin only on the far side.
///
/// An archived project still answers every other call here — including this one, which is how it comes
/// back — so there is nothing to guard against on this side.
pub async fn set_project_archived(project: &str, archived: bool) -> Result<(), RequestError> {
    let request = SetProjectArchivedInputModel {
        project: project.to_string(),
        archived,
    };

    handle_http_empty(authed("/api/projects/v1/archived/set", HttpVerb::Post, request).await).await
}

/// Replaces the whole set — which is how the screen works, and means this side never has to diff.
pub async fn set_members(project: &str, members: Vec<String>) -> Result<(), RequestError> {
    let request = SetProjectMembersInputModel {
        project: project.to_string(),
        members,
    };

    handle_http_empty(authed("/api/projects/v1/members/set", HttpVerb::Post, request).await).await
}
