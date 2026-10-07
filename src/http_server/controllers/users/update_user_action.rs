use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::users::UpdateUserInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/users/v1/update",
    controller: "Users",
    summary: "Change a name, an admin flag or disable someone",
    description: "Admin only. The email is not editable — it is the identity, and every assignment and comment authorship points at it. Disabling ends their open sessions immediately rather than letting a live tab keep working, and leaves their tasks and comments exactly where they are.",
    input_data: "UpdateUserInputModel",
    result: [
        {status_code: 200, description: "Updated"},
        {status_code: 400, description: "Not on the roster"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct UpdateUserAction {
    app: Arc<AppContext>,
}

impl UpdateUserAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &UpdateUserAction,
    input_data: UpdateUserInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::update_user(
        &action.app,
        &input_data.email,
        &input_data.name,
        input_data.admin,
        input_data.disabled,
    )
    .await
    .map_err(bad_request)?;

    // Nothing to revoke: their next request re-reads the row, finds it disabled and comes back 401 — and
    // the same check runs on the WebSocket handshake, so an open tab stops receiving updates too.

    HttpOutput::Empty.into_ok_result(true)
}
