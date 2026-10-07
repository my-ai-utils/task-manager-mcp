use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::users::UsersResponse;

use crate::app::AppContext;
use crate::mappers::user_to_response;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/users/v1/list",
    controller: "Users",
    summary: "The roster",
    description: "Admin only. Feeds the Users screen and the membership checkboxes in Projects setup. `admin_from_settings` marks the people whose admin right comes from the service settings rather than their row — the UI shows that as granted-elsewhere instead of an editable checkbox that would silently do nothing.",
    result: [
        {status_code: 200, description: "Everyone on the roster", model: "UsersResponse"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct ListUsersAction {
    app: Arc<AppContext>,
}

impl ListUsersAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &ListUsersAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    let roster = action.app.board.read().users();

    let mut users = Vec::with_capacity(roster.len());
    for user in roster.iter() {
        let from_settings = action.app.is_admin_in_settings(&user.email).await;
        users.push(user_to_response(user, from_settings));
    }

    HttpOutput::as_json(UsersResponse { users }).into_ok_result(true)
}
