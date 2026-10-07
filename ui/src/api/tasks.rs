use flurl::HttpVerb;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::tasks::{
    FindTaskInputModel, FindTaskResponse, GetTasksInputModel, MoveTaskInputModel, TaskResponse,
    TasksResponse,
};

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response};

/// Read a board.
pub async fn get_tasks(
    project: &str,
    include_archived: bool,
) -> Result<TasksResponse, RequestError> {
    let request = GetTasksInputModel {
        project: project.to_string(),
        include_archived: Some(include_archived),
    };

    handle_http_response(authed("/api/tasks/v1/list", HttpVerb::Post, request).await).await
}

/// Look one task up by its handle — `RMS-42`.
///
/// Always the server: a task closed more than seven days ago is not in the board read, and a handle names
/// its own project, which may not be the one on screen. A miss comes back as `not_found` text rather than
/// as an error, because "there is no RMS-999" is an answer, not a failure.
pub async fn find_task(query: &str) -> Result<FindTaskResponse, RequestError> {
    let request = FindTaskInputModel {
        query: query.to_string(),
    };

    handle_http_response(authed("/api/tasks/v1/find", HttpVerb::Post, request).await).await
}

/// The answer a lookup by id would have given, assembled from what is already on screen.
///
/// No round trip: whichever screen is drawing the task already holds it and the project it is on, so the
/// dialog opens instantly. `archived` is false by definition of being drawn — the one caller that can show
/// archived work (a goal's own list) is showing it deliberately, and the dialog does not use the flag to
/// decide anything, only to say so.
pub fn find_task_locally(task: &TaskResponse, project: &ProjectResponse) -> FindTaskResponse {
    FindTaskResponse {
        task: Some(task.clone()),
        goal: None,
        project: project.prefix.clone(),
        project_name: project.name.clone(),
        archived: false,
        not_found: String::new(),
    }
}

/// Move a task to another column — the one thing the board changes about a task.
///
/// Everything else still goes through `/mcp`: the text, the type, the assignee, the thread. This is the
/// narrowest opening that makes a board behave like a board, and the server applies exactly the rules an
/// MCP status change obeys — including that landing in `done` needs a comment, which is what `comment` is
/// for. The author is not passed: this side has a session, so the server signs it with whoever dragged the
/// card.
pub async fn move_task(
    task_id: &str,
    status: &str,
    comment: Option<&str>,
) -> Result<(), RequestError> {
    let request = MoveTaskInputModel {
        task_id: task_id.to_string(),
        status: status.to_string(),
        comment: comment.map(|itm| itm.to_string()),
    };

    handle_http_empty(authed("/api/tasks/v1/move", HttpVerb::Post, request).await).await
}
