use my_http_utils::macros::{MyHttpInput, MyHttpObjectStructure};
use serde::{Deserialize, Serialize};

// Never put `///` doc comments on fields of a struct deriving MyHttpInput or
// MyHttpObjectStructure: the macro's attribute parser panics with `Somehow we got Punct here: =`.

// Everything the UI needs about the signed-in user.
//
// `name` may be empty — the UI then falls back to the email. `is_admin` is the resolved answer
// (the flag on the user row OR membership of the settings admin list), not the stored flag: the UI
// gates Projects setup and Users on this one value and should not have to know where the right came
// from.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct MeResponse {
    pub email: String,
    pub name: String,
    pub is_admin: bool,
}

// Where to send the browser to sign in.
//
// The URL is built server-side because it carries the `client_id` and the `redirect_uri` from the
// service settings, and the client has no business knowing either.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GoogleAuthUrlResponse {
    pub url: String,
}

/// The cookie the session travels in.
///
/// **A cookie rather than a header, and `HttpOnly` rather than local storage**, which buys three things a
/// header could not. The browser attaches it to requests our code does not make — an `<img>` or an `<iframe>`
/// pointed at a document's bytes, and the WebSocket handshake — so neither needs the token spelled into its
/// url, where it would land in browser history, in `Referer`, in proxy logs, and in any link somebody copied.
/// `HttpOnly` then puts it out of reach of script entirely, which matters here because this product renders
/// html somebody else uploaded.
pub const SESSION_COOKIE: &str = "task_manager_session";

// There is no cookie for which board is open, and there was one for exactly one release. The session belongs in
// a cookie because the browser has to attach it to requests our code does not make; which board somebody is
// looking at is the opposite kind of fact — this side never reads it, because every request that acts on a
// project NAMES the project it acts on: in the body, or in the `/raw/{prefix}/{path}` url. So it is a
// preference of one browser's, it lives in that browser's local storage, and nothing about it is shared
// vocabulary — which is why this file no longer has a constant for it.

// What a completed sign-in hands back.
//
// The token is in the response as well as in a `Set-Cookie` on it, because the browser is not the only thing
// that signs in — and because a client that wants to keep one has somewhere to read it from. What the browser
// uses is the cookie.
#[derive(Serialize, Deserialize, MyHttpObjectStructure, Clone, Debug, PartialEq)]
pub struct GoogleCallbackResponse {
    pub token: String,
}

// Google hands the browser back to us with a one-time `code`; `state` is the CSRF token we issued
// with the auth URL and expect to see returned unchanged.
#[derive(MyHttpInput)]
// Body, not query, and that is load-bearing. Both values are base64-ish and regularly contain a `+`; a
// `+` in a QUERY value means a space, so sending them as query parameters mangled the state on roughly
// six sign-ins in ten — the ones whose state happened to contain one — and surfaced as a 401 that looked
// random. A JSON body carries the bytes verbatim and has no such rule.
pub struct GoogleCallbackInputModel {
    #[http_body(name: "code", description: "One-time authorization code from Google")]
    pub code: String,
    #[http_body(name: "state", description: "The CSRF token issued with the auth URL")]
    pub state: String,
}
