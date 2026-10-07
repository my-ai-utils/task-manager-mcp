use std::sync::Arc;

use service_sdk::macros::use_my_http_server;
use task_manager_shared::auth::MeResponse;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "POST",
    route: "/api/auth/v1/me",
    controller: "Auth",
    summary: "Who am I",
    description: "The signed-in person and whether they may configure the product. `is_admin` is the resolved answer — the flag on their row or the settings admin list — so the UI gates Projects setup and Users on this one value.",
    result: [
        {status_code: 200, description: "The signed-in person", model: "MeResponse"},
        {status_code: 401, description: "Not authenticated"},
    ]
)]
pub struct MeAction {
    app: Arc<AppContext>,
}

impl MeAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

async fn handle_request(
    action: &MeAction,
    ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    let user = crate::auth::resolve_auth_user(&action.app, ctx).await?;

    HttpOutput::as_json(MeResponse {
        email: user.email,
        name: user.name,
        is_admin: user.is_admin,
    })
    .into_ok_result(true)
}
