use wasm_bindgen::JsCast;

const LAST_PROJECT_KEY: &str = "task_manager_project";

/// Which board was open last, by PREFIX.
///
/// **Local storage rather than a cookie, and the prefix rather than the id.** A cookie was the wrong shape
/// twice over: the server never read it — every request that acts on a project NAMES the project it acts on, in
/// the body or in the `/raw/{prefix}/{path}` url, so there was nothing for a cookie to tell it — and it rode on
/// every single request regardless, including the ones fetching a document's bytes. This is a preference of this
/// browser's, and it belongs where the other one already lives, beside the open folders of the document tree.
///
/// The prefix because that is what a person calls a board and what every MCP tool names one by; the internal id
/// stays inside the process. The screens hold ids, so they translate against the project list they have just
/// loaded — which is also what makes a board somebody lost access to fall through rather than leave the screen
/// on nothing.
///
/// It is a PREFERENCE and never an authority: every request that acts on a project checks membership of the
/// project it names, so hand-editing this value opens nothing its owner could not already open. That is what
/// makes it safe for the client to write.
pub fn save_last_project(prefix: &str) {
    super::local_storage::set(LAST_PROJECT_KEY, prefix);
}

/// The remembered prefix, or `None` when this browser has not settled on a board yet.
///
/// An empty string reads as `None`: a value written before a prefix was known would otherwise be a board no
/// project can match, which is a slower way of saying nothing.
pub fn get_last_project() -> Option<String> {
    super::local_storage::get(LAST_PROJECT_KEY)
        .map(|itm| itm.trim().to_string())
        .filter(|itm| !itm.is_empty())
}

/// Expire the cookie an older build kept this preference in.
///
/// The same job [`super::clear_session_token`] does for the token that used to live in local storage, in the
/// other direction. Without it a browser that ran the previous build sends `task_manager_project=…` on every
/// request for a year — on the api calls, on the WebSocket handshake and on every document's bytes — saying
/// something nothing reads any more.
///
/// `path=/` because that is the scope it was written at: a browser treats a cookie of the same name at another
/// path as a different cookie, and clearing the wrong one leaves the stale value in place while looking like it
/// worked.
pub fn clear_project_cookie() {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };

    if let Ok(document) = document.dyn_into::<web_sys::HtmlDocument>() {
        let _ = document.set_cookie("task_manager_project=; path=/; max-age=0; SameSite=Lax");
    }
}
