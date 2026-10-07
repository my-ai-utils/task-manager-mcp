use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::tasks::{FindTaskInputModel, FindTaskResponse};

use crate::app::AppContext;
use crate::mappers::task_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/tasks/v1/find",
    controller: "Tasks",
    summary: "Look one task up by its id",
    description: "Resolves a handle — RMS-42, or RMS-000042 — to the exact task, whichever project it belongs to. Worth its own endpoint rather than a search of the loaded board for two reasons: work closed more than seven days ago is not in the board read at all, and a handle names its own project, which may not be the one on screen. An id that resolves to a project the caller may not see answers as not found rather than as a refusal, because saying \"no access\" would confirm the task exists.",
    input_data: "FindTaskInputModel",
    result: [
        {status_code: 200, description: "The hit, or a message saying why there is none", model: "FindTaskResponse"},
        {status_code: 401, description: "Not authenticated"},
    ]
)]
pub struct FindTaskAction {
    app: Arc<AppContext>,
}

impl FindTaskAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

/// Nothing found, with the reason in words.
fn nothing(reason: String) -> FindTaskResponse {
    FindTaskResponse {
        task: None,
        goal: None,
        project: String::new(),
        project_name: String::new(),
        archived: false,
        not_found: reason,
    }
}

async fn handle_request(
    action: &FindTaskAction,
    input_data: FindTaskInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let user = crate::auth::resolve_auth_user(&action.app, ctx).await?;

    let board = action.app.board.read();

    // A goal first, and only when the query is spelled as one: `RMS-G7` cannot be a task, so there is
    // nothing to fall through to. This is the only route to a goal that has aged off the screen.
    if crate::board::parse_goal_handle(input_data.query.trim()).is_some() {
        return found_goal(action, &board, &user, input_data.query.trim());
    }

    // `resolve_task` already says what would fix a miss — a bad shape, an unknown prefix with the known
    // ones listed, or a number that is not on that board. Passing its message through beats inventing a
    // flatter one here.
    let found = match crate::scripts::resolve_task(&board, &input_data.query) {
        Ok(found) => found,
        Err(err) => {
            // A bare `RMS-7` may be a goal's number: one counter serves both kinds, so a query that no
            // task answers is worth asking the goals about before reporting nothing.
            if let Some(parsed) = crate::board::parse_task_handle(input_data.query.trim()) {
                if let Some(project) = board.get_project_by_prefix(&parsed.prefix) {
                    if board.get_goal_including_deleted(&project.id, parsed.number).is_some() {
                        let handle =
                            crate::board::compose_goal_handle(&project.prefix, parsed.number);
                        return found_goal(action, &board, &user, &handle);
                    }
                }
            }

            return HttpOutput::as_json(nothing(err)).into_ok_result(true);
        }
    };

    // Not a 403: refusing would confirm that RMS-42 exists, which is the one thing somebody probing ids
    // for a project they cannot see would learn. "Not found" tells them nothing either way.
    if !user.is_admin && !found.project.is_member(&user.email) {
        return HttpOutput::as_json(nothing(format!(
            "no task {} you can see",
            input_data.query.trim()
        )))
        .into_ok_result(true);
    }

    let archived = board.is_archived(&found.task);
    let task = task_to_response(&found.task, &found.project, &board);

    HttpOutput::as_json(FindTaskResponse {
        task: Some(task),
        goal: None,
        project: found.project.prefix.clone(),
        project_name: found.project.name.clone(),
        archived,
        not_found: String::new(),
    })
    .into_ok_result(true)
}

/// Answer with a goal.
///
/// Membership is checked the same way and refused the same way — as "not found" rather than as a 403,
/// because a refusal would confirm that the goal exists, which is the one thing somebody probing ids for a
/// board they cannot see would learn.
fn found_goal(
    action: &FindTaskAction,
    board: &crate::board::BoardInner,
    user: &crate::auth::AuthUser,
    handle: &str,
) -> Result<HttpOkResult, HttpFailResult> {
    let _ = action;

    let found = match crate::scripts::resolve_goal_by_handle(board, handle) {
        Ok(found) => found,
        Err(err) => return HttpOutput::as_json(nothing(err)).into_ok_result(true),
    };

    if !user.is_admin && !found.project.is_member(&user.email) {
        return HttpOutput::as_json(nothing(format!("no goal {handle} you can see")))
            .into_ok_result(true);
    }

    HttpOutput::as_json(FindTaskResponse {
        task: None,
        goal: Some(crate::mappers::goal_to_response(
            &found.goal,
            &found.project,
            board,
        )),
        project: found.project.prefix.clone(),
        project_name: found.project.name.clone(),
        archived: board.is_goal_archived(&found.goal),
        not_found: String::new(),
    })
    .into_ok_result(true)
}
