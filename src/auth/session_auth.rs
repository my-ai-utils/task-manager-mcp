use std::sync::Arc;

use crate::app::AppContext;
use crate::http_server::errors::{forbidden, unauthorized};

service_sdk::macros::use_my_http_server!();

/// The resolved caller.
pub struct AuthUser {
    pub email: String,
    pub name: String,
    /// The answer, not the stored flag: the `admin` column on the user row **or** membership of the
    /// settings admin list. Callers gate on this and should not have to know which one it came from.
    pub is_admin: bool,
}

/// Pull the session token out of the request: the cookie first, the `Authorization` header second.
///
/// **The cookie is where it lives now.** A header cannot be attached to a request our code does not make, and
/// three of them matter here: the `<img>` and the `<iframe>` that fetch a document's bytes, and the WebSocket
/// handshake. All three used to need the token spelled into their url — where it lands in history, in
/// `Referer` and in proxy logs — and the cookie is what removes that.
///
/// The header is still accepted, and deliberately: at the moment this shipped every signed-in browser held a
/// token in local storage and no cookie, and refusing it would have signed everybody out mid-session for
/// nothing. It costs three lines and can go once no live token predates the change.
pub fn extract_session(ctx: &HttpContext) -> Option<String> {
    let from_cookie = ctx
        .request
        .get_cookies()
        .get(task_manager_shared::auth::SESSION_COOKIE)
        .map(str::trim)
        .filter(|itm| !itm.is_empty())
        .map(|itm| itm.to_string());

    if from_cookie.is_some() {
        return from_cookie;
    }

    extract_bearer(ctx)
}

/// Pull the session token out of the `Authorization` header.
pub fn extract_bearer(ctx: &HttpContext) -> Option<String> {
    let raw = ctx
        .request
        .get_headers()
        .try_get_case_insensitive_as_str("Authorization")
        .ok()
        .flatten()?;

    let raw = raw.trim();
    let token = raw
        .strip_prefix("Bearer ")
        .or_else(|| raw.strip_prefix("bearer "))
        .unwrap_or(raw)
        .trim();

    if token.is_empty() {
        None
    } else {
        Some(token.to_string())
    }
}

/// Resolve the caller from the session token.
///
/// The user row is re-read on every request rather than trusted from the session, so disabling someone
/// or taking their admin flag away takes effect immediately instead of when their token happens to
/// expire. Reads are from memory, so this costs nothing.
pub async fn resolve_auth_user(
    app: &Arc<AppContext>,
    ctx: &HttpContext,
) -> Result<AuthUser, HttpFailResult> {
    let Some(token) = extract_session(ctx) else {
        return Err(unauthorized("Not authenticated"));
    };

    resolve_auth_user_from_token(app, &token).await
}

/// The same resolution, from a token pulled out of somewhere other than the standard request path — the
/// WebSocket handshake, which is not an `HttpContext`. It is the same token, checked the same way; only the
/// envelope differs.
pub async fn resolve_auth_user_from_token(
    app: &Arc<AppContext>,
    token: &str,
) -> Result<AuthUser, HttpFailResult> {
    let Some(session) = crate::auth::SessionToken::parse(token, &app.session_key) else {
        return Err(unauthorized("Not authenticated"));
    };

    let admin_in_settings = app.is_admin_in_settings(&session.email).await;

    match app.board.read().get_user(&session.email) {
        Some(user) => {
            if user.disabled {
                return Err(unauthorized("Account is disabled"));
            }

            Ok(AuthUser {
                email: user.email.clone(),
                name: user.name.clone(),
                is_admin: user.admin || admin_in_settings,
            })
        }
        // No row, but the settings list names them: that is the bootstrap case, and refusing it would
        // leave an empty database with no way in. Everyone else needs a row.
        None if admin_in_settings => Ok(AuthUser {
            email: session.email.clone(),
            name: String::new(),
            is_admin: true,
        }),
        None => Err(unauthorized("Not authenticated")),
    }
}

/// For the endpoints that configure the product. Hiding a menu entry in the UI is cosmetic — this is
/// what actually enforces it.
pub async fn require_admin(
    app: &Arc<AppContext>,
    ctx: &HttpContext,
) -> Result<AuthUser, HttpFailResult> {
    let user = resolve_auth_user(app, ctx).await?;

    if !user.is_admin {
        return Err(forbidden("Admin only"));
    }

    Ok(user)
}

/// Resolve the caller and check they may look at this board, in one call.
///
/// One function rather than two because every board read needs both, and because a `Result` whose error
/// is an `HttpFailResult` and whose success is `()` is a shape clippy rightly objects to — the error
/// dwarfs the value it is wrapped with.
pub async fn require_project_access(
    app: &Arc<AppContext>,
    ctx: &HttpContext,
    project_id: &str,
) -> Result<AuthUser, HttpFailResult> {
    let user = resolve_auth_user(app, ctx).await?;
    check_project_access(app, user, project_id)
}

/// The project a request names by PREFIX, once the caller has been shown to be allowed it.
///
/// **Every project-scoped endpoint on this boundary starts here.** The prefix is the only name a project has
/// on the wire — see `ProjectResponse` in the shared crate — so an action's first two steps are always the
/// same: resolve what the caller sent, then check membership of what it resolved to. Doing them together is
/// what stops an action doing the second against a project it never resolved, or skipping it altogether.
///
/// The whole project comes back rather than its id, because every caller needs more of it: the columns to
/// validate a status against, the prefix to compose a handle from, the id to speak to the board with.
///
/// **The resolver's own error message is deliberately dropped.** It names every prefix that exists, which is
/// right for MCP — an agent that mistyped a prefix is entitled to the list — and wrong here: a browser already
/// holds the boards it may see, and somebody who is not on a board must not be handed a map of the ones they
/// are not on.
pub async fn require_project_by_prefix(
    app: &Arc<AppContext>,
    ctx: &HttpContext,
    prefix: &str,
) -> Result<Arc<crate::board::ProjectModel>, HttpFailResult> {
    // In a block of its own: the board lock is not held across the await below.
    let project = {
        let board = app.board.read();
        crate::scripts::resolve_project_by_prefix(&board, prefix).ok()
    };

    let Some(project) = project else {
        return Err(crate::http_server::errors::not_found("No such project"));
    };

    require_project_access(app, ctx, &project.id).await?;

    Ok(project)
}

fn check_project_access(
    app: &Arc<AppContext>,
    user: AuthUser,
    project_id: &str,
) -> Result<AuthUser, HttpFailResult> {
    if user.is_admin {
        return Ok(user);
    }

    let is_member = app
        .board
        .read()
        .get_project(project_id)
        .map(|project| project.is_member(&user.email))
        .unwrap_or(false);

    if is_member {
        Ok(user)
    } else {
        // The same message a missing project gets, so probing ids cannot map out boards the caller has
        // no access to.
        Err(forbidden("No access to this project"))
    }
}
