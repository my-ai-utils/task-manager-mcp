use std::sync::Arc;

use service_sdk::macros::use_my_http_server;

use crate::app::AppContext;

use_my_http_server!();

#[http_route(
    method: "GET",
    route: "/api/system/v1/ping",
    controller: "System",
    summary: "Liveness",
    description: "Public. Says the process is up and its state is loaded — not that Postgres is reachable, since after startup nothing reads from it.",
    result: [
        {status_code: 200, description: "Alive"},
    ]
)]
pub struct PingAction {
    _app: Arc<AppContext>,
}

impl PingAction {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { _app: app }
    }
}

async fn handle_request(
    _action: &PingAction,
    _ctx: &HttpContext,
) -> Result<HttpOkResult, HttpFailResult> {
    HttpOutput::as_text("OK".to_string()).into_ok_result(true)
}
