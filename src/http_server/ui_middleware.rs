use service_sdk::my_http_server::{
    HttpContext, HttpFailResult, HttpOkResult, HttpServerMiddleware, StaticFilesMiddleware,
};

/// The first path segment of everything this server answers itself.
///
/// * `api` — the REST controllers, and service-sdk's own `/api/isalive`;
/// * `mcp`, `ws`, `raw` — the three middlewares registered beside this one;
/// * `swagger`, `metrics` — what service-sdk mounts on every HTTP server it builds.
///
/// A list of what is NOT the browser's rather than of what is: the client adds a screen far more often
/// than the server adds a surface, and a screen that had to be registered here to be reachable by a
/// reload would be forgotten the first time.
const SERVER_SEGMENTS: [&str; 6] = ["api", "mcp", "ws", "raw", "swagger", "metrics"];

/// Serves the browser its bundle — `index.html`, the wasm, the stylesheet — out of `./wwwroot`.
///
/// **This is what makes the product one container.** The client used to be an image of its own behind a
/// second port, with a reverse proxy sending `/api`, `/ws` and `/mcp` one way and everything else the
/// other. It is a folder of static files; this server is already listening on the origin those files
/// call back to. So they are served from here, the layout my-no-sql-server has.
///
/// **A wrapper around the stock middleware, and the wrapping is the point.** The client is a single-page
/// app: `/goals` and `/releases` are routes inside the wasm, not files, so a path that names no file has
/// to answer with `index.html` or a reload of any screen but the first is a 404. `StaticFilesMiddleware`
/// does that — for EVERY request it is shown, whatever the method and whatever the path. And service-sdk
/// runs custom middlewares BEFORE the controllers, where my-no-sql-server, which builds its server by
/// hand, puts the static files last. Registered bare, it would therefore answer `POST /api/tasks/v1/list`
/// with a page of HTML and a 200, and the board would break in the most confusing way available: every
/// call succeeding and none of them parsing.
///
/// So two things are decided here before it is asked: only a `GET` (or the `HEAD` of one) is a browser
/// fetching a page, and a path under one of [`SERVER_SEGMENTS`] is the server's own to answer — including
/// to answer "no such route", which must stay a 404 and not become the login screen.
pub struct UiMiddleware {
    static_files: StaticFilesMiddleware,
}

impl UiMiddleware {
    pub fn new() -> Self {
        Self {
            static_files: StaticFilesMiddleware::new()
                .add_index_file("index.html")
                // Any route of the client. The wasm reads the address and draws the screen it names.
                .set_not_found_file("index.html".to_string())
                // Every file goes out with an ETag and `Cache-Control: no-cache`: the browser keeps it
                // and asks whether it is still current, which costs a 304 instead of the whole wasm on
                // each visit — and, since it does ask, a deploy is picked up by the next reload rather
                // than whenever a cache happens to expire. Without this the bundle is re-downloaded in
                // full every time, because a response with no validator cannot be revalidated.
                .with_etag(),
        }
    }
}

impl Default for UiMiddleware {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl HttpServerMiddleware for UiMiddleware {
    async fn handle_request(
        &self,
        ctx: &mut HttpContext,
    ) -> Option<Result<HttpOkResult, HttpFailResult>> {
        if !is_page_fetch(ctx.request.method.as_str()) {
            return None;
        }

        if !is_ui_path(ctx.request.http_path.as_str()) {
            return None;
        }

        self.static_files.handle_request(ctx).await
    }
}

/// Whether a request is a browser fetching something to show. Everything this product's client SENDS is
/// a `POST`, so the method alone separates the two — with the two downloads that are `GET` on purpose
/// living under `/api`, which the path check covers.
fn is_page_fetch(method: &str) -> bool {
    method.eq_ignore_ascii_case("GET") || method.eq_ignore_ascii_case("HEAD")
}

/// Whether a path is the client's — a file of the bundle, or a route inside it.
///
/// Decided on the first segment and case-insensitively, the way the routes themselves are matched. The
/// whole segment, not a prefix of it: `/apiary` would be a screen, were there ever one.
fn is_ui_path(path: &str) -> bool {
    let first_segment = path
        .trim_start_matches('/')
        .split(['/', '?'])
        .next()
        .unwrap_or_default();

    !SERVER_SEGMENTS
        .iter()
        .any(|itm| itm.eq_ignore_ascii_case(first_segment))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundle's own files and every screen the client has — including one it does not have yet, which
    /// is the reason the list is of the server's paths and not of the client's.
    #[test]
    fn the_bundle_and_every_screen_are_the_clients() {
        for path in [
            "/",
            "",
            "/index.html",
            "/favicon.ico",
            "/assets/app.css",
            "/wasm/task-manager-ui_bg.wasm",
            "/goals",
            "/releases",
            "/documents",
            "/settings/column-templates",
            "/authorized",
            "/a-screen-nobody-has-written-yet",
            // A whole segment decides it, so a screen may START with the name of a surface.
            "/apiary",
            "/wsx",
        ] {
            assert!(is_ui_path(path), "{path:?} should be served the bundle");
        }
    }

    /// Answering any of these with `index.html` is how an API call would come back as a page of HTML and
    /// a 200 — and for `swagger`, which service-sdk mounts AFTER the custom middlewares, how the API's own
    /// documentation would turn into the login screen.
    #[test]
    fn what_the_server_answers_itself_is_left_to_it() {
        for path in [
            "/api/tasks/v1/list",
            "/api/system/v1/ping",
            "/api/projects/v1/export",
            "/api/no-such-route",
            "/api",
            "/API/tasks/v1/list",
            "/mcp",
            "/ws",
            "/raw/TM/docs/page.html",
            "/raw",
            "/swagger/index.html",
            "/metrics",
        ] {
            assert!(!is_ui_path(path), "{path:?} is the server's to answer");
        }
    }

    /// Only a fetch. The stock middleware answers a POST with the page as readily as a GET, and every
    /// write this product's browser makes is a POST.
    #[test]
    fn only_a_get_is_a_page_being_fetched() {
        assert!(is_page_fetch("GET"));
        assert!(is_page_fetch("HEAD"));

        for method in ["POST", "PUT", "DELETE", "OPTIONS", "PATCH"] {
            assert!(!is_page_fetch(method), "{method} is not a page fetch");
        }
    }

    /// Stands where service-sdk puts the controllers: AFTER every custom middleware. Whatever reaches it
    /// was left alone by the one under test.
    struct Controllers;

    const FROM_THE_CONTROLLERS: &str = "answered by the controllers";

    #[async_trait::async_trait]
    impl HttpServerMiddleware for Controllers {
        async fn handle_request(
            &self,
            _ctx: &mut HttpContext,
        ) -> Option<Result<HttpOkResult, HttpFailResult>> {
            Some(
                service_sdk::my_http_server::HttpOutput::as_text(FROM_THE_CONTROLLERS.to_string())
                    .into_ok_result(false),
            )
        }
    }

    /// One answer, read whole: the status, the ETag if there was one, and the body.
    async fn fetch(
        request: flurl::FlUrl,
        post: bool,
    ) -> (u16, Option<String>, Vec<u8>) {
        let response = if post {
            request
                .post(flurl::body::HttpRequestBody::from_raw_data(
                    b"{}".to_vec(),
                    Some("application/json"),
                ))
                .await
        } else {
            request.get().await
        };

        let mut response = response.expect("the request should be answered");

        let status = response.get_status_code();
        let etag = response
            .get_header("etag")
            .ok()
            .flatten()
            .map(|itm| itm.to_string());
        let body = response
            .get_body_as_slice()
            .await
            .expect("the body should arrive")
            .to_vec();

        (status, etag, body)
    }

    /// **The whole arrangement, over a real socket, in the order service-sdk builds it.**
    ///
    /// This is the one part of serving the client from the server that can go wrong silently: the page
    /// loads either way, and what breaks is every call it then makes. So it is proved against the bundle
    /// that is actually committed — `./wwwroot`, which `cargo test` runs beside — rather than against a
    /// fixture, with a stand-in for the controllers placed where service-sdk puts the real ones.
    #[tokio::test]
    async fn the_bundle_is_served_and_the_servers_own_paths_are_not_shadowed() {
        use std::sync::Arc;

        use service_sdk::my_http_server::MyHttpServer;

        let index = std::fs::read("wwwroot/index.html")
            .expect("wwwroot/index.html is committed — run ./build-ui.sh if it is not there");

        // A port nobody holds: asked of the OS, read, and given back for the server to take.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();

        let mut server = MyHttpServer::new(std::net::SocketAddr::from(([127, 0, 0, 1], port)));
        server.add_middleware(Arc::new(UiMiddleware::new()));
        server.add_middleware(Arc::new(Controllers));
        server.start(
            Arc::new(service_sdk::rust_extensions::AppStates::create_initialized()),
            service_sdk::my_logger::LOGGER.clone(),
        );

        let origin = format!("http://127.0.0.1:{port}");
        let at = |path: &str| flurl::FlUrl::new(format!("{origin}{path}"));

        // The listener is bound by a spawned task, so the first requests may be refused. Waited for
        // rather than slept for.
        let mut up = false;

        for _ in 0..100 {
            if at("/api/ping").get().await.is_ok() {
                up = true;
                break;
            }

            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        assert!(up, "the server did not start listening");

        // The page itself, and a route that exists only inside the wasm: both are `index.html`, or a
        // reload of any screen but the first is a 404 — and Google's redirect to `/authorized` with it.
        for path in ["/", "/index.html", "/releases", "/authorized?code=x&state=y", "/settings/kinds"] {
            let (status, etag, body) = fetch(at(path), false).await;

            assert_eq!(status, 200, "GET {path}");
            assert_eq!(body, index, "GET {path} should be the client's page");
            assert!(etag.is_some(), "GET {path} should carry an ETag to revalidate with");
        }

        // A file of the bundle, as itself.
        let css = std::fs::read("wwwroot/assets/app.css").expect("the stylesheet is in the bundle");
        let (status, etag, body) = fetch(at("/assets/app.css"), false).await;

        assert_eq!(status, 200);
        assert_eq!(body, css);

        // Asked again with what it was given, it is not sent twice. This is what `with_etag` is for:
        // the wasm is megabytes, and without a validator it is downloaded whole on every visit.
        let (status, _, body) = fetch(
            at("/assets/app.css").with_header("If-None-Match", etag.expect("an ETag on a file")),
            false,
        )
        .await;

        assert_eq!(status, 304);
        assert!(body.is_empty());

        // Everything the server answers itself gets through to it, GET included: the two downloads
        // under `/api` are GETs, a mistyped route there has to stay the server's 404, and swagger is
        // mounted behind this middleware.
        for path in ["/api/system/v1/ping", "/api/no-such-route", "/swagger/index.html", "/ws", "/mcp"] {
            let (status, _, body) = fetch(at(path), false).await;

            assert_eq!(status, 200, "GET {path}");
            assert_eq!(
                body,
                FROM_THE_CONTROLLERS.as_bytes(),
                "GET {path} is the server's to answer"
            );
        }

        // And a POST is never a page — not under `/api`, and not at a path that would be a screen. The
        // stock middleware would have answered both with `index.html` and a 200.
        for path in ["/api/tasks/v1/list", "/goals"] {
            let (status, _, body) = fetch(at(path), true).await;

            assert_eq!(status, 200, "POST {path}");
            assert_eq!(
                body,
                FROM_THE_CONTROLLERS.as_bytes(),
                "POST {path} must reach the server"
            );
        }
    }
}
