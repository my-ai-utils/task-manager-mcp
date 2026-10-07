use std::cell::RefCell;

use futures::channel::mpsc::{Sender, channel};

// The write half of the WebSocket, reachable from anywhere in the client.
//
// A channel rather than the socket itself: the socket is owned by the task reading it, and wasm is
// single-threaded, so a `thread_local` sender is both sound and the least machinery that works. It is
// also why a `watch` sent before the socket finishes opening is not lost — it waits in the channel.
thread_local! {
    static WS_SENDER: RefCell<Option<Sender<String>>> = const { RefCell::new(None) };
}

/// Room for a few pending messages. The client only ever sends a `watch`, and only when somebody picks a
/// project from a dropdown, so this cannot back up in practice.
const CAPACITY: usize = 8;

/// Create the channel and hand the receiving half to the socket task.
pub fn install_ws_sender() -> futures::channel::mpsc::Receiver<String> {
    let (sender, receiver) = channel::<String>(CAPACITY);

    WS_SENDER.with(|cell| {
        *cell.borrow_mut() = Some(sender);
    });

    receiver
}

/// Tell the server which board to push about.
///
/// Silently does nothing when the socket was never opened — the board still renders, it just will not
/// repaint by itself, and the grey dot in the header is what says so.
pub fn watch_project(prefix: &str) {
    let payload = format!("{{\"watch\":\"{prefix}\"}}");

    WS_SENDER.with(|cell| {
        if let Some(sender) = cell.borrow_mut().as_mut() {
            let _ = sender.try_send(payload);
        }
    });
}
