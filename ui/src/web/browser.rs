/// Write to the browser console.
///
/// Hand-written because `dioxus-utils` has no logging helper — its `js` module is storage, focus, sleep,
/// reload and `GlobalAppSettings`. Used only where something went wrong that the screen cannot usefully
/// show: a WebSocket that dropped, a `/me` that failed on the transport.
pub fn console_log(message: &str) {
    web_sys::console::log_1(&wasm_bindgen::JsValue::from_str(message));
}

/// Leave the app entirely — a full browser navigation, not a route push.
///
/// Two callers, and both genuinely need to leave: the Google consent page is not ours to route to, and
/// signing out has to throw away every signal in the app, including the WebSocket task and the cached
/// `/me`, which a route push would leave running underneath the login screen.
pub fn navigate_to(url: &str) {
    if let Some(window) = web_sys::window() {
        let _ = window.location().set_href(url);
    }
}

/// The shareable address of one task, absolute — the thing somebody pastes into a chat.
///
/// `?search=` because that is the door this app already has: the box takes a handle, and Home follows one
/// found in the URL at mount to its board and opens the task. So a link is the search box written down,
/// there is no second parameter to keep working, and closing the dialog leaves the reader on the right
/// board rather than on a screen the link invented.
///
/// Nothing is encoded because nothing in a handle can need it — `PREFIX-42` is ASCII alphanumerics, an
/// underscore and a dash, all of which are literal in a query value.
///
/// The origin comes from the page rather than from configuration: this client never knows a base URL,
/// which is the same reason it calls the API on relative paths.
pub fn task_link(handle: &str) -> String {
    let origin = web_sys::window()
        .and_then(|window| window.location().origin().ok())
        .unwrap_or_default();

    format!("{origin}/?search={handle}")
}

/// Put text on the clipboard, and say whether the browser would take it.
///
/// `navigator.clipboard` exists only in a secure context — https, or localhost while `dx serve` runs —
/// which is both of the ways this app is ever reached. It is checked anyway: wasm-bindgen types the getter
/// as always-there, so on a page served over plain http the call below would be a method on `undefined`,
/// and a JS throw crossing wasm takes the whole board down rather than one button. `false` lets the caller
/// show the link instead.
///
/// The promise is dropped, not awaited. It resolves after the write, and a caller that has already said
/// "copied" has nothing left to do with the answer — the failure it would report is denied clipboard
/// permission, which no browser asks about for a write the reader clicked.
///
/// Call it from the click handler itself rather than from a spawned future: the write is only allowed while
/// the gesture that asked for it is still being handled, and Safari is the one that enforces it.
pub fn copy_to_clipboard(text: &str) -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };

    let clipboard = window.navigator().clipboard();

    if clipboard.is_undefined() {
        return false;
    }

    let _ = clipboard.write_text(text);
    true
}
