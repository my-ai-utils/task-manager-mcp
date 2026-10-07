use service_sdk::my_http_server::cookies::Cookie;
use task_manager_shared::auth::SESSION_COOKIE;

use super::SESSION_TTL_HOURS;

/// The cookie a completed sign-in sets.
///
/// Every flag on it is load-bearing:
///
/// * **`HttpOnly`** — script cannot read it. That is the whole reason this beats local storage here: the
///   product renders html that somebody else uploaded, and a token script can read is a token an uploaded page
///   can take. It is also why the client no longer needs to know the token at all.
/// * **`Secure`** — it never travels in clear. The service is behind a reverse proxy that terminates TLS, so
///   this costs nothing in production; it does mean a plain-http origin will not keep a session, which is the
///   correct trade for a credential.
/// * **`SameSite`** — another site cannot make the browser send it. Our REST surface only reads, so the
///   exposure was small, but a cookie that rides on cross-site requests is how CSRF starts.
/// * **`Path=/`** — the raw document route lives outside `/api`, and a cookie scoped to `/api` would not be
///   sent to it, which is the one request that most needs it.
///
/// Its lifetime matches the token's own: a cookie outliving what it carries would leave the browser sending a
/// credential the server has already stopped accepting, and the failure would read as "randomly signed out".
pub fn issue_session_cookie(token: String) -> Cookie {
    Cookie::new(SESSION_COOKIE, token)
        .set_http_only()
        .set_secure()
        .set_same_site()
        .set_path("/")
        .set_max_age(SESSION_TTL_HOURS as u64 * 60 * 60)
}

/// The cookie that signs a browser out: the same name and the same scope, emptied and expired at once.
///
/// The scope has to match what set it — a browser treats a cookie of the same name at another path as a
/// different cookie, and clearing the wrong one leaves the session in place while looking like it worked.
pub fn clear_session_cookie() -> Cookie {
    Cookie::new(SESSION_COOKIE, String::new())
        .set_http_only()
        .set_secure()
        .set_same_site()
        .set_path("/")
        .set_max_age(0)
}
