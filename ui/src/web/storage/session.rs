const SESSION_TOKEN_KEY: &str = "task_manager_session_token";

/// Forget a session token this browser stored before sessions moved into a cookie.
///
/// **The only thing left of the old scheme, and it exists to finish removing it.** The session is now an
/// `HttpOnly` cookie: the browser attaches it to every request by itself, including the ones our code does not
/// make — the `<img>` and `<iframe>` that fetch a document's bytes, and the WebSocket handshake — and script
/// cannot read it, which is what a page somebody else uploaded must not be able to do.
///
/// So nothing writes a token here any more, and nothing reads one. What remains is clearing what an older
/// build left behind, so a stale token does not sit in local storage for a year after it stopped meaning
/// anything.
pub fn clear_session_token() {
    super::local_storage::delete(SESSION_TOKEN_KEY);
}

/// Which folders of a project's document tree were left open.
///
/// Local storage, like the remembered board beside it in [`super::save_last_project`] and for the same reason:
/// the server has no use for either, so neither has any business riding on every request the way a cookie
/// does — and this one can be dozens of paths.
///
/// Keyed per project, because a path in one project names nothing in another. Newline-separated, which is safe
/// because a document path cannot contain a newline: it is normalised server-side into slash-separated
/// segments with the whitespace trimmed.
pub fn get_expanded_folders(project: &str) -> std::collections::HashSet<String> {
    super::local_storage::get(&expanded_key(project))
        .map(|raw| {
            raw.split('\n')
                .filter(|itm| !itm.is_empty())
                .map(|itm| itm.to_string())
                .collect()
        })
        .unwrap_or_default()
}

pub fn set_expanded_folders(project: &str, folders: &std::collections::HashSet<String>) {
    let joined: Vec<&str> = folders.iter().map(|itm| itm.as_str()).collect();
    super::local_storage::set(&expanded_key(project), &joined.join("\n"));
}

fn expanded_key(project: &str) -> String {
    format!("task_manager_documents_expanded_{project}")
}
