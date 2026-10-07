use std::sync::Arc;
use std::time::Duration;

use service_sdk::my_http_server::web_sockets::*;

use crate::app::AppContext;

/// The `/ws` endpoint Home holds open.
///
/// One message shape in each direction, and both are as small as they can be:
///
/// * client -> server: `{"watch":"RMS"}` — the board's PREFIX, sent on connect and again whenever the
///   project dropdown changes, so switching boards does not need a reconnect. A prefix and not an internal
///   id, like every other name of a project on this boundary — see `ProjectResponse` in the shared crate.
/// * server -> client: `{"boardSnapshot":{…}}` — the board itself, with `{"projectChanged":"RMS"}` beside
///   it for a tab that predates snapshots. Never a delta: with no second copy of the state on the client
///   there is nothing that can drift out of sync.
pub struct HomeWsCallbacks {
    app: Arc<AppContext>,
}

impl HomeWsCallbacks {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

#[async_trait::async_trait]
impl MyWebSocketCallback for HomeWsCallbacks {
    async fn connected(
        &self,
        my_web_socket: Arc<MyWebSocket>,
        http_request: MyWebSocketHttpRequest,
        _disconnect_timeout: Duration,
    ) -> Result<(), WebSocketConnectedFail> {
        // The browser WebSocket API cannot send custom headers — but it DOES send cookies on the handshake,
        // which is what the session now travels in. So the token no longer has to be spelled into the url,
        // where it landed in browser history and in proxy logs.
        //
        // The other two are still read, and in this order: an `Authorization` header for anything that is not
        // a browser, and the old `token=` query for a tab that was already open when this shipped. The query
        // form can go once no live session predates the change.
        let token = http_request
            .get_headers()
            .get("Cookie")
            .and_then(|value| value.to_str().ok())
            .and_then(|cookies| {
                cookies.split(';').find_map(|pair| {
                    let (name, value) = pair.split_once('=')?;

                    if name.trim() == task_manager_shared::auth::SESSION_COOKIE {
                        Some(value.trim().to_string())
                    } else {
                        None
                    }
                })
            })
            .filter(|itm| !itm.is_empty())
            .or_else(|| {
                http_request
                    .get_headers()
                    .get("Authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(|value| {
                        value
                            .trim()
                            .strip_prefix("Bearer ")
                            .unwrap_or(value.trim())
                            .to_string()
                    })
            })
            .or_else(|| {
                http_request.get_uri().query().and_then(|query| {
                    query
                        .split('&')
                        .find_map(|pair| pair.strip_prefix("token="))
                        .map(|itm| itm.to_string())
                })
            })
            .unwrap_or_default();

        let Some(session) = crate::auth::SessionToken::parse(&token, &self.app.session_key) else {
            return Err(WebSocketConnectedFail {
                reason: "Not authenticated".to_string(),
                // Not logged: an expired token on a reconnecting tab is routine, and logging it would
                // turn every overnight laptop into noise.
                write_to_logs: false,
            });
        };

        let admin_in_settings = self.app.is_admin_in_settings(&session.email).await;
        let user = self.app.board.read().get_user(&session.email);

        // Disabling somebody has to close the door here too, not only on the REST side.
        let is_admin = match &user {
            Some(user) if user.disabled => {
                return Err(WebSocketConnectedFail {
                    reason: "Account is disabled".to_string(),
                    write_to_logs: false,
                });
            }
            Some(user) => user.admin || admin_in_settings,
            None if admin_in_settings => true,
            None => {
                return Err(WebSocketConnectedFail {
                    reason: "Not authenticated".to_string(),
                    write_to_logs: false,
                });
            }
        };

        self.app
            .subscribers
            .add(my_web_socket, session.email, is_admin);

        Ok(())
    }

    async fn disconnected(&self, my_web_socket: &MyWebSocket) {
        self.app.subscribers.remove(my_web_socket.id);
    }

    async fn on_message(&self, my_web_socket: Arc<MyWebSocket>, message: WsMessage) {
        let WsMessage::Text(payload) = message else {
            // Anything that is not text is not part of this protocol. Ignored rather than treated as an
            // error: a ping frame or a stray binary message is not worth disconnecting a board over.
            return;
        };

        let Some(connection) = self.app.subscribers.get_by_id(my_web_socket.id) else {
            return;
        };

        let Some(prefix) = parse_watch(payload.as_str()) else {
            connection
                .send_error("expected {\"watch\":\"RMS\"} — a project prefix")
                .await;
            return;
        };

        // Resolved here and held as an ID for the life of the subscription, which is the one place on this
        // side where that is the better half of the trade: the push comes from `notify_project_changed`,
        // which knows the project by id, and a subscription pinned to the id keeps working through a prefix
        // rename instead of going quiet until the tab is reloaded.
        //
        // Membership is re-checked on every subscribe, not only at connect: a tab left open across a
        // membership change must not keep receiving a board it may no longer see.
        let resolved = {
            let board = self.app.board.read();

            crate::scripts::resolve_project_by_prefix(&board, &prefix)
                .ok()
                .filter(|project| connection.is_admin || project.is_member(&connection.email))
                .map(|project| project.id.clone())
        };

        let Some(project_id) = resolved else {
            // The same answer for a prefix nobody holds and a board this caller may not see: probing must
            // not map out the boards somebody is not on.
            connection.send_error("no access to this project").await;
            return;
        };

        connection.watch(project_id);
    }
}

/// Read `{"watch":"RMS"}` — a project PREFIX.
///
/// Parsed with serde_json rather than by hand, which costs nothing and cannot be wrong about an escape in a
/// quoted value. Empty reads as no watch at all rather than as a board called "": the caller is told, and
/// nothing is silently subscribed.
fn parse_watch(payload: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(payload).ok()?;
    let prefix = value.get("watch")?.as_str()?.trim();

    if prefix.is_empty() {
        None
    } else {
        Some(prefix.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watch_message_is_read() {
        assert_eq!(
            parse_watch("{\"watch\":\"RMS\"}"),
            Some("RMS".to_string()),
            "a prefix, which is how the client names a board everywhere else too"
        );
    }

    #[test]
    fn anything_else_is_refused_rather_than_guessed_at() {
        for payload in [
            "",
            "not json",
            "{}",
            "{\"watch\":\"\"}",
            "{\"watch\":null}",
            "{\"watch\":42}",
            "{\"other\":\"x\"}",
        ] {
            assert_eq!(parse_watch(payload), None, "{payload:?} should not parse");
        }
    }
}
