use std::sync::Arc;

use ahash::AHashMap;
use arc_swap::ArcSwap;
use parking_lot::Mutex;
use service_sdk::my_http_server::web_sockets::MyWebSocket;

use super::HomeConnection;

type ConnectionsMap = AHashMap<i64, Arc<HomeConnection>>;

/// Every Home currently connected.
///
/// Same shape as the board state and for the same reason: read on every mutation, written only when a
/// browser tab opens or closes. `ArcSwap` means a notification takes no lock at all, and the
/// `Mutex<()>` only stops two concurrent connects from losing one of the two.
///
/// Sends happen against a snapshot with no lock held — one client on a stalled socket must not be able
/// to hold up every other watcher, and a `parking_lot` guard is `!Send`, so the compiler refuses the
/// mistake rather than leaving it to review.
pub struct ProjectSubscribers {
    connections: ArcSwap<ConnectionsMap>,
    write_lock: Mutex<()>,
}

impl ProjectSubscribers {
    pub fn new() -> Self {
        Self {
            connections: ArcSwap::from_pointee(AHashMap::new()),
            write_lock: Mutex::new(()),
        }
    }

    pub fn add(&self, ws: Arc<MyWebSocket>, email: String, is_admin: bool) -> Arc<HomeConnection> {
        let id = ws.id;
        let connection = Arc::new(HomeConnection::new(ws, email, is_admin));

        let _guard = self.write_lock.lock();
        let mut next: ConnectionsMap = (**self.connections.load()).clone();
        next.insert(id, connection.clone());
        self.connections.store(Arc::new(next));

        connection
    }

    pub fn remove(&self, id: i64) -> Option<Arc<HomeConnection>> {
        let _guard = self.write_lock.lock();
        let mut next: ConnectionsMap = (**self.connections.load()).clone();
        let removed = next.remove(&id);
        self.connections.store(Arc::new(next));

        removed
    }

    pub fn get_by_id(&self, id: i64) -> Option<Arc<HomeConnection>> {
        self.connections.load().get(&id).cloned()
    }

    pub fn amount(&self) -> usize {
        self.connections.load().len()
    }

    /// Send one payload to everyone watching this board and entitled to see it.
    ///
    /// Called from [`crate::app::AppContext::notify_project_changed`], which is where the payload is built —
    /// after the change is in Postgres **and** in memory. Sending any earlier would push a board that does
    /// not show the change yet.
    ///
    /// `members` is re-checked here and not only at subscribe time, and it matters more than it used to: a
    /// signal to go and re-read was harmless to send to somebody who had just lost access, because the read
    /// itself would refuse them. A snapshot IS the board, so the check has to happen before the send.
    pub async fn push_to_watchers(&self, project_id: &str, payload: &str, members: &[String]) {
        let connections = self.connections.load_full();

        for connection in connections.values() {
            if !connection.is_watching(project_id) {
                continue;
            }

            let allowed = connection.is_admin
                || members
                    .iter()
                    .any(|member| member.eq_ignore_ascii_case(&connection.email));

            if allowed {
                connection.send_payload(payload).await;
            }
        }
    }
}

impl Default for ProjectSubscribers {
    fn default() -> Self {
        Self::new()
    }
}
