use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::users::CreateUserInputModel;

use crate::app::AppContext;
use crate::http_server::errors::bad_request;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/users/v1",
    controller: "Users",
    summary: "Put someone on the roster",
    description: "Admin only. The email must be the Google account they will sign in with — it is the identity, and it is what a task assignment and a comment author point at. Being on the roster is what allows a sign-in at all; it grants no project on its own.",
    input_data: "CreateUserInputModel",
    result: [
        {status_code: 200, description: "Created"},
        {status_code: 400, description: "Bad email, or already on the roster"},
        {status_code: 401, description: "Not authenticated"},
        {status_code: 403, description: "Admin only"},
    ]
)]
pub struct CreateUserAction {
    app: Arc<AppContext>,
}

impl CreateUserAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &CreateUserAction,
    input_data: CreateUserInputModel,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    crate::auth::require_admin(&action.app, ctx).await?;

    crate::scripts::create_user(
        &action.app,
        &input_data.email,
        &input_data.name,
        input_data.admin,
    )
    .await
    .map_err(bad_request)?;

    HttpOutput::Empty.into_ok_result(true)
}
