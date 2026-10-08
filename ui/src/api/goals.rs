use flurl::HttpVerb;
use task_manager_shared::goals::{GetGoalsInputModel, GoalsResponse, SetGoalColorInputModel};

use crate::models::RequestError;

use super::{authed, handle_http_empty, handle_http_response};

/// Read a project's goals.
///
/// Reads only, like the board: goals are opened, renamed and closed through `/mcp`.
///
/// `include_archived` is the switch `get_tasks` has, and means the same thing. Without it a goal closed
/// longer ago than the project's window is left out — that is the list the Goals screen draws. With it the
/// history comes too, which is what a screen needs when it is asked for ONE goal by its id and that goal
/// may have shipped months ago: a release names the goal it carried for as long as the release exists.
pub async fn get_goals(
    project: &str,
    include_archived: bool,
) -> Result<GoalsResponse, RequestError> {
    let request = GetGoalsInputModel {
        project: project.to_string(),
        include_archived: Some(include_archived),
    };

    handle_http_response(authed("/api/goals/v1/list", HttpVerb::Post, request).await).await
}

/// Recolour a goal.
///
/// The one thing the browser writes about the board, and only because a colour is presentation rather than
/// state — the same argument that puts a task type's colour behind a mouse. Everything else about a goal
/// still arrives through `/mcp`.
pub async fn set_goal_color(
    project: &str,
    goal: &str,
    color: &str,
) -> Result<(), RequestError> {
    let request = SetGoalColorInputModel {
        project: project.to_string(),
        goal: goal.to_string(),
        color: color.to_string(),
    };

    handle_http_empty(authed("/api/goals/v1/color", HttpVerb::Post, request).await).await
}
