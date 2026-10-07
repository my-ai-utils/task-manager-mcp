use std::sync::Arc;

use arc_swap::ArcSwap;
use service_sdk::my_http_server::web_sockets::{MyWebSocket, WsMessage};
use task_manager_shared::ws::ServerWsPayload;

/// One connected Home.
///
/// `watching` is swappable rather than fixed at connect time: Home has a project dropdown, and
/// picking another board should re-target the subscription without tearing down the socket.
pub struct HomeConnection {
    pub ws: Arc<MyWebSocket>,
    pub email: String,
    pub is_admin: bool,
    watching: ArcSwap<String>,
}

impl HomeConnection {
    pub fn new(ws: Arc<MyWebSocket>, email: String, is_admin: bool) -> Self {
        Self {
            ws,
            email,
            is_admin,
            watching: ArcSwap::from_pointee(String::new()),
        }
    }

    pub fn watch(&self, project_id: String) {
        self.watching.store(Arc::new(project_id));
    }

    pub fn is_watching(&self, project_id: &str) -> bool {
        self.watching.load().as_str() == project_id
    }

    /// Send one already-serialised payload.
    ///
    /// Serialised by the caller, once, and handed to every watcher: a board goes out to everyone looking at
    /// it, and encoding the same tasks once per connection is work that scales with the wrong number.
    pub async fn send_payload(&self, payload: &str) {
        self.ws
            .send_message(std::iter::once(WsMessage::Text(payload.to_string().into())))
            .await;
    }

    pub async fn send_error(&self, message: &str) {
        let payload = serde_json::to_string(&ServerWsPayload::error(message))
            .unwrap_or_else(|_| "{\"error\":\"error\"}".to_string());

        self.send_payload(&payload).await;
    }
}
