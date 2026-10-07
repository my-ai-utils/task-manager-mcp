use std::sync::Arc;

use app::AppContext;

mod app;
mod auth;
mod board;
mod documents;
mod github;
mod http_server;
mod mappers;
mod mcp;
mod postgres;
mod scripts;
mod settings;
mod subscribers;

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

#[tokio::main]
async fn main() {
    let settings_reader = settings::SettingsReader::new("~/.task-manager").await;
    let settings_reader = Arc::new(settings_reader);

    let mut service_context = service_sdk::ServiceContext::new(settings_reader.clone()).await;

    let app = Arc::new(AppContext::new(settings_reader).await);

    // Read the whole product into memory BEFORE the HTTP server starts serving. Every read path assumes
    // the board is loaded; doing this in the background would serve an empty board for the first few
    // hundred milliseconds after every deploy, which looks exactly like data loss.
    scripts::load_state(&app).await;

    // After the board, because it walks every project's connections — and NOT awaited, because a
    // repository that is slow to answer must not hold up the port opening. Every mirror starts empty and
    // fills within a few seconds of the service being up.
    github::run_puller(app.clone());

    // Three surfaces on one HTTP server, in one process, over one copy of the state:
    //   /api/*  — reads for the UI and the configuration CRUD
    //   /mcp    — every task mutation
    //   /ws     — the invalidation push that tells Home to re-read
    //
    // That co-location is what makes the WebSocket fan-out a function call rather than a message bus,
    // and it is why this service runs as a single instance.
    //
    // And a fourth thing that is not a surface: everything else is the browser's bundle, served out of
    // `./wwwroot` by this same server — which is what makes the product one container.
    let mcp_middleware = Arc::new(mcp::build_middleware(app.clone()));

    service_context.configure_http_server(move |builder| {
        let ws_middleware = Arc::new(
            service_sdk::my_http_server::web_sockets::MyWebsocketMiddleware::new(
                "/ws",
                Arc::new(http_server::ws::HomeWsCallbacks::new(app.clone())),
                service_sdk::my_logger::LOGGER.clone(),
            ),
        );

        // Before the controllers, and outside `/api` on purpose: a framed html document resolves its relative
        // assets against this route, so the shape of the url is part of the contract. See the middleware.
        builder.register_custom_middleware(Arc::new(
            http_server::RawDocumentsMiddleware::new(app.clone()),
        ));

        builder.register_custom_middleware(ws_middleware);
        builder.register_custom_middleware(mcp_middleware.clone());

        // LAST of the custom middlewares, and it would not be enough on its own: service-sdk runs every
        // one of these before the controllers, so the bundle is behind a wrapper that leaves the server's
        // own paths alone. See `UiMiddleware` for what happens without it.
        builder.register_custom_middleware(Arc::new(http_server::UiMiddleware::new()));

        http_server::build_controllers(&app, builder);
    });

    service_context.start_application().await;
}
