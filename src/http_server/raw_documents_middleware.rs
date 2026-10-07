use std::sync::Arc;

use service_sdk::my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpOutput, HttpServerMiddleware, WebContentType,
};
use task_manager_shared::documents::{is_html_content_type, parse_raw_document_url};

use crate::app::AppContext;
use crate::http_server::errors::not_found;
use crate::scripts::{DocumentBody, body_of, content_type_of};

/// Serves a document's bytes at `/raw/{prefix}/{path}`.
///
/// **A middleware rather than an action, and the path form rather than a query, for one reason: a framed html
/// page asks for its own assets with relative urls.** The browser resolves those against the address the page
/// came from — served as `/raw/TM/docs/page.html`, a `style.css` beside it resolves to
/// `/raw/TM/docs/style.css` and arrives. From `?project=TM&id=…` it would resolve back onto the api route and
/// arrive as nothing. A path of arbitrary depth is not something the routing macro can express, which is what
/// makes this a middleware.
///
/// **The session is a cookie, so the url carries no token.** That is the other half of the change: this url is
/// what "open full screen" hands somebody, and a token in it would travel into browser history, `Referer`
/// headers and proxy logs. As a cookie it opens for the reader only if they are signed in and on that board.
///
/// Registered before the controllers; anything that is not this route returns `None` and falls through.
pub struct RawDocumentsMiddleware {
    app: Arc<AppContext>,
}

impl RawDocumentsMiddleware {
    pub fn new(app: Arc<AppContext>) -> Self {
        Self { app }
    }
}

#[async_trait::async_trait]
impl HttpServerMiddleware for RawDocumentsMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        let (prefix, path) = parse_raw_document_url(ctx.request.http_path.as_str())?;

        Some(serve(&self.app, ctx, &prefix, &path).await)
    }
}

async fn serve(
    app: &Arc<AppContext>,
    ctx: &HttpContext,
    prefix: &str,
    path: &str,
) -> Result<HttpOkResult, HttpFailResult> {
    // The project comes from the URL and membership is checked against it, which is what makes the url safe to
    // hand around: naming a board does not open it.
    let project_id = {
        let board = app.board.read();

        match crate::scripts::resolve_project_by_prefix(&board, prefix) {
            Ok(project) => project.id.clone(),
            // The same message a forbidden project gets: probing prefixes must not map out boards the caller
            // cannot see.
            Err(_) => return Err(not_found("No such document")),
        }
    };

    crate::auth::require_project_access(app, ctx, &project_id).await?;

    // A path under the reserved root is a file in a connected repository — no row, and never was one. It is
    // served from the mirror on disk instead, through the same access check and with the same headers, so an
    // image in a repository draws in the viewer exactly as one of the project's own does.
    let (content_type, content) = if task_manager_shared::github::is_github_path(path) {
        serve_from_mirror(app, prefix, path).await?
    } else if let Some(id) = document_id_in(path) {
        serve_by_id(app, &project_id, id, path).await?
    } else {
        serve_from_documents(app, &project_id, path).await?
    };

    // HTML is the one type served here that a browser executes, and it would execute in OUR origin. A document
    // is uploaded by an agent through `/mcp`, so an unsandboxed page would be running somebody else's script
    // beside the signed-in session.
    //
    // `sandbox allow-scripts` puts the response in an opaque origin: scripts and styles still run, so the page
    // renders as the page it is, but it cannot reach our API as the signed-in user. `allow-scripts` WITHOUT
    // `allow-same-origin` is the combination that matters — together the two let a framed document remove the
    // sandbox itself.
    //
    // On the RESPONSE and not only on the frame, because "open full screen" loads this url directly in a tab,
    // where a frame's `sandbox` attribute does not exist.
    let sandbox = if is_html_content_type(&content_type) {
        Some("sandbox allow-scripts".to_string())
    } else {
        None
    };

    HttpOutput::from_builder()
        .set_content(content)
        .set_content_type(WebContentType::Raw(with_charset(content_type)))
        .add_header_if_some("Content-Security-Policy", sandbox)
        // Without it a browser sniffs a type for anything whose declared one looks wrong — which is how a
        // document stored as text/plain gets executed as html, straight past the check above.
        .add_header("X-Content-Type-Options", "nosniff".to_string())
        // The url is stable but the document behind it is not: an agent can rewrite it while somebody reads.
        .add_header(
            "Cache-Control",
            "no-store, no-cache, must-revalidate, max-age=0".to_string(),
        )
        .into_ok_result(false)
}

/// A text type with `charset=utf-8` on it, and everything else untouched.
///
/// **Everything this service stores as text is UTF-8, and a browser told nothing does not assume that.** Its
/// default for `text/html` and `text/css` is the locale's encoding, so a mirrored page with a Cyrillic word in
/// it arrives as mojibake — which reads as a broken document rather than as a missing header.
///
/// Only on the wire, never in what is stored: the charset is how the bytes travel, not what the document IS,
/// and putting it in the column would leave every screen and every tool comparing `text/markdown` against
/// something that no longer equals it.
///
/// A type that already carries a charset is left exactly as it arrived — the caller said it, and a second
/// parameter would be malformed.
fn with_charset(content_type: String) -> String {
    if !task_manager_shared::documents::content_type_needs_charset(&content_type) {
        return content_type;
    }

    if content_type.to_lowercase().contains("charset") {
        return content_type;
    }

    format!("{content_type}; charset=utf-8")
}

/// The document id a `document/<id>` path names, or `None` for a path that names a document instead.
///
/// **The second half of the reference vocabulary, and the only part of it this route did not already
/// serve.** A file in a connected repository is named by its path, which this route has always taken; a
/// document of the project's own has an ID that survives it being moved, and a reference that stored the
/// path would go stale the moment somebody reorganised a folder.
///
/// Exactly one segment after the word, because an id holds no separator. That is also what keeps the
/// ambiguity narrow: `document/spec/a.md` is a document of the project's own in a folder called
/// `document`, reads as one, and is served as one.
fn document_id_in(path: &str) -> Option<&str> {
    let rest = path.strip_prefix(task_manager_shared::documents::RAW_OWN_SEGMENT)?;
    let id = rest.strip_prefix('/')?;

    match id.is_empty() || id.contains('/') {
        true => None,
        false => Some(id),
    }
}

/// One of the project's own documents, named by its id.
///
/// **Falls back to the path when the id names nothing**, which is not leniency for its own sake: a project
/// that already had a document at `document/<something>` had a working url for it before this branch
/// existed, and that url has to keep working. A miss on both is the same "No such document" either way.
async fn serve_by_id(
    app: &Arc<AppContext>,
    project_id: &str,
    id: &str,
    path: &str,
) -> Result<(String, Vec<u8>), HttpFailResult> {
    let telemetry = service_sdk::my_telemetry::MyTelemetryContext::create_empty();

    let Some(row) = app.documents_repo.get_by_id(id, &telemetry).await else {
        return serve_from_documents(app, project_id, path).await;
    };

    // The id found the row, so the project it belongs to has to be checked against the one the url named —
    // the access check above is about the project that was NAMED, not the one the document is on.
    if row.project_id != project_id {
        return Err(not_found("No such document"));
    }

    let content_type = content_type_of(row.content_type.as_deref(), &row.doc_path);

    let content = match body_of(&row) {
        DocumentBody::Text(text) => text.into_bytes(),
        DocumentBody::Binary(bytes) => bytes,
    };

    Ok((content_type, content))
}

/// One of the project's own documents, as a content type and bytes.
///
/// By PATH rather than by id, because the path is what the url carries — and it is what a relative link
/// inside a framed page resolves to, which is the whole point of this route.
async fn serve_from_documents(
    app: &Arc<AppContext>,
    project_id: &str,
    path: &str,
) -> Result<(String, Vec<u8>), HttpFailResult> {
    let telemetry = service_sdk::my_telemetry::MyTelemetryContext::create_empty();

    let Some(row) = app
        .documents_repo
        .get_by_path(project_id, path, &telemetry)
        .await
    else {
        return Err(not_found("No such document"));
    };

    let content_type = content_type_of(row.content_type.as_deref(), &row.doc_path);

    // Text and bytes both come out as bytes here: a text document served raw is what makes "download" work on
    // one, and its Content-Type says what it is.
    let content = match body_of(&row) {
        DocumentBody::Text(text) => text.into_bytes(),
        DocumentBody::Binary(bytes) => bytes,
    };

    Ok((content_type, content))
}

/// One file out of a connected repository's mirror.
///
/// Read through `read_mirror_document`, which resolves the path against the mirror's own entry list, so
/// what can be served here is exactly what the mirror published — not whatever a path happens to reach on
/// the container's disk.
///
/// A miss says "No such document", the same as a document that is not there: whether a path is missing
/// because the repository never had it or because the mirror has not pulled yet is a distinction for the
/// Documents screen to draw, not for a url anybody can type.
async fn serve_from_mirror(
    app: &Arc<AppContext>,
    prefix: &str,
    path: &str,
) -> Result<(String, Vec<u8>), HttpFailResult> {
    let row = crate::scripts::read_mirror_document(app, prefix, path)
        .await
        .map_err(|_| not_found("No such document"))?;

    let content_type = content_type_of(row.content_type.as_deref(), &row.doc_path);

    let content = match body_of(&row) {
        DocumentBody::Text(text) => text.into_bytes(),
        DocumentBody::Binary(bytes) => bytes,
    };

    Ok((content_type, content))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A page's own assets are what this is for: a stylesheet with a Cyrillic comment and a script with a
    /// Cyrillic string both arrive as UTF-8 or as mojibake, and nothing but this header decides which.
    #[test]
    fn text_travels_with_a_charset_and_nothing_else_does() {
        assert_eq!(
            with_charset("text/css".to_string()),
            "text/css; charset=utf-8"
        );
        assert_eq!(
            with_charset("text/html".to_string()),
            "text/html; charset=utf-8"
        );
        assert_eq!(
            with_charset("application/json".to_string()),
            "application/json; charset=utf-8"
        );

        // Bytes carry their own encoding or none at all, and a charset on them is meaningless.
        assert_eq!(with_charset("image/png".to_string()), "image/png");
        assert_eq!(with_charset("font/woff2".to_string()), "font/woff2");
        assert_eq!(
            with_charset("application/octet-stream".to_string()),
            "application/octet-stream"
        );
    }

    /// The word plus ONE segment is an id; anything else is a path that happens to start with it, and a
    /// project that already had a folder called `document` keeps every url it had.
    #[test]
    fn only_one_segment_after_the_word_is_an_id() {
        assert_eq!(
            document_id_in("document/01K2C4Q0S1T2U3V4W5X6Y7Z8"),
            Some("01K2C4Q0S1T2U3V4W5X6Y7Z8")
        );

        // A folder called `document`, which is a document of the project's own and is served by path.
        assert_eq!(document_id_in("document/spec/a.md"), None);
        assert_eq!(document_id_in("document"), None);
        assert_eq!(document_id_in("document/"), None);

        // A folder whose name merely starts with the same letters is nothing to do with this.
        assert_eq!(document_id_in("documents/a.md"), None);
        assert_eq!(document_id_in("docs/a.md"), None);
    }

    /// One a caller already spelled out is left exactly as it is — a second parameter would be malformed,
    /// and theirs is the one that was meant.
    #[test]
    fn a_charset_that_is_already_there_is_not_doubled() {
        assert_eq!(
            with_charset("text/html; charset=windows-1251".to_string()),
            "text/html; charset=windows-1251"
        );
        assert_eq!(
            with_charset("text/plain; CHARSET=utf-8".to_string()),
            "text/plain; CHARSET=utf-8"
        );
    }
}
