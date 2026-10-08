use dioxus::prelude::*;
use dioxus_utils::RenderState;
use futures::{SinkExt, StreamExt};
use reqwasm::websocket::{Message, futures::WebSocket};
use task_manager_shared::auth::MeResponse;

mod api;
mod dialogs;
mod models;
mod states;
mod templates;
mod views;
mod web;

use models::ServerWsMessage;
use states::AppState;

#[derive(Routable, PartialEq, Clone)]
pub enum AppRoute {
    // The board, and what is being looked for on it. The search is in the URL so a board somebody is staring
    // at can be handed over as a link — including `?search=RMS-42`, which lands on that task's project
    // whatever the recipient was last looking at.
    //
    // `Option<String>` rather than `String`: nothing to search for writes no argument at all, so the bare
    // board stays `/?` instead of `/?search=`.
    #[route("/?:search")]
    Home { search: Option<String> },
    // Where Google sends the browser back. The path is NOT free to choose: it has to match the
    // `google-redirect-uri` secret, which in turn has to match the Google console byte for byte.
    //
    // A route rather than a server redirect so the token exchange and storing the token happen in the same
    // place the rest of the client lives.
    //
    // The query arguments MUST be declared here, and they arrive as PROPS — reading
    // `window.location().search()` in a hook is too late. A route that does not name them gets the query
    // stripped: the address bar ends up at a bare `/authorized` and the search string comes back empty,
    // which reads exactly like "Google sent nothing". That is what the first version did.
    //
    // Google also appends `iss`, `scope`, `authuser` and `prompt`, which we neither send nor read —
    // undeclared query arguments are ignored, so they are harmless.
    #[route("/authorized?:code&:state&:error")]
    AuthCallback {
        code: String,
        state: String,
        error: String,
    },
    // The same work as Home, from the other end: goals with their tasks folded underneath. No query
    // arguments — nothing here is searched, and which project is showing is remembered rather than linked.
    #[route("/goals")]
    Goals {},
    // What has gone out, newest first. No query arguments, for the reason Goals has none: which project
    // is showing is remembered rather than linked.
    #[route("/releases")]
    Releases {},
    // One release, on a page of its own — the address somebody is handed. Unlike the list above, WHICH
    // release is the whole of the address: the board's prefix, then the release's id (`RMS-R12`) or its
    // bare number. The prefix comes first and on its own although the id repeats it, so the link reads
    // without knowing how an id is put together and access is decided on the half that names the board.
    // The shape is also written where links are BUILT — `release_page_path` in the shared crate — and a
    // test in the page's state holds the two together.
    #[route("/release/:project/:release")]
    ReleasePage { project: String, release: String },
    // The project's documents: the tree on the left, whatever is selected on the right. The selection is IN
    // the url so a document can be linked to — an agent can say "see TM/docs/design.md" as an address, and a
    // reload lands back on it. Modelled on the file browser in `remote-development-mcp`.
    #[route("/documents?:selected")]
    Documents { selected: String },
    #[route("/projects-setup")]
    ProjectsSetup {},
    #[route("/users")]
    Users {},
    // Bare `/settings` lands on the first section; the section is part of the route so every area is
    // linkable and Back works between them.
    #[route("/settings")]
    Settings {},
    #[route("/settings/:section")]
    SettingsSection { section: String },
    #[route("/logout")]
    Logout {},
}

fn main() {
    // Before anything renders, and it is the whole of what the old scheme leaves behind: the board a browser
    // was on used to be a cookie, and a cookie nothing reads still rides on every request — including every
    // document's bytes — until it expires a year later. See `storage::save_last_project`.
    crate::web::storage::clear_project_cookie();

    dioxus::LaunchBuilder::new().launch(|| {
        rsx! {
            document::Link { rel: "icon", href: asset!("/public/favicon.ico") }
            document::Meta {
                name: "viewport",
                content: "width=device-width, initial-scale=1.0",
            }
            Router::<AppRoute> {}
        }
    });
}

/// Everything below the router shares one `AppState`, and every screen goes through [`Shell`].
///
/// `Shell` owns the one question every screen needs answered first — is anybody signed in — so no
/// individual view has to handle "not yet asked", and none of them can forget to.
#[component]
fn Shell(active: &'static str, children: Element) -> Element {
    let app_state = use_context_provider(|| Signal::new(AppState::default()));

    // A context of its own rather than a field of `AppState` — see `dialogs::DialogState` for why.
    use_context_provider(|| Signal::new(crate::dialogs::DialogState::None));
    // How the last submit went, for the dialogs whose request the router makes — see `DialogFeedback`.
    use_context_provider(|| Signal::new(crate::dialogs::DialogFeedback::default()));

    let app_state_ra = app_state.read();

    // Read in the RENDER body, which is what subscribes this scope to it — the previous version asked
    // inside `use_future`, which spawns once and tracks nothing.
    let me = match get_me(app_state, &app_state_ra) {
        Ok(me) => me,
        Err(element) => return element,
    };

    if me.is_none() {
        return rsx! {
            crate::views::login::RenderLogin {}
        };
    }

    rsx! {
        crate::templates::ContentPanel { active, {children} }
        // Last, and outside the content panel: a dialog is an overlay over whatever screen opened it,
        // and it is mounted once here so no screen has to remember to.
        crate::dialogs::RenderDialog {}
    }
}

/// Whoever is signed in, asked once per page load.
///
/// `/me` is the only way to find out: the token in localStorage may be expired, encrypted with a rotated
/// key, or belong to somebody since disabled — all of which look identical from here and are all answered
/// by asking. `Ok(None)` is "nobody", which is data rather than a failure.
fn get_me(
    mut app_state: Signal<AppState>,
    app_state_ra: &AppState,
) -> Result<Option<&MeResponse>, Element> {
    match app_state_ra.signed_in.as_ref() {
        RenderState::None => {
            spawn(async move {
                app_state.write().signed_in.set_loading();

                match crate::api::get_me().await {
                    Ok(me) => {
                        // A token the server will not accept is a token worth throwing away, or every
                        // later request carries it and is refused all over again.
                        if me.is_none() {
                            crate::web::storage::clear_session_token();
                        }

                        app_state.write().signed_in.set_loaded(me);
                    }
                    // A transport failure is not "signed out", but the login screen is the only thing
                    // this shell has to offer either way — so it is logged, where the real problem is
                    // findable, and then treated as nobody.
                    Err(err) => {
                        crate::web::console_log(format!("/me failed: {err}").as_str());
                        app_state.write().signed_in.set_loaded(None);
                    }
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(me) => Ok(me.as_ref()),
        // Not reachable — a failed `/me` is stored as "nobody" above — but if that ever changes, the
        // login screen is still the only useful thing to put in front of somebody.
        RenderState::Error(_) => Err(rsx! {
            crate::views::login::RenderLogin {}
        }),
    }
}

/// The whole-window spinner the shell shows while `/me` is out. Not the in-page one: there is no page yet.
fn render_loading() -> Element {
    rsx! {
        div { class: "full-screen",
            div { class: "loading-note", "Loading…" }
        }
    }
}

#[component]
fn Home(search: Option<String>) -> Element {
    rsx! {
        Shell { active: "home",
            crate::views::home::RenderHome { search }
        }
    }
}

#[component]
fn Goals() -> Element {
    rsx! {
        Shell { active: "goals",
            crate::views::goals::RenderGoals {}
        }
    }
}

#[component]
fn Releases() -> Element {
    rsx! {
        Shell { active: "releases",
            crate::views::releases::RenderReleases {}
        }
    }
}

/// Under the Releases tab: it is one of them, and the tab is the way back to the rest.
#[component]
fn ReleasePage(project: String, release: String) -> Element {
    rsx! {
        Shell { active: "releases",
            crate::views::release::RenderReleasePage { project, release }
        }
    }
}

#[component]
fn Documents(selected: String) -> Element {
    rsx! {
        Shell { active: "documents",
            crate::views::documents::RenderDocuments { selected }
        }
    }
}

#[component]
fn ProjectsSetup() -> Element {
    rsx! {
        Shell { active: "projects",
            crate::views::projects_setup::RenderProjectsSetup {}
        }
    }
}

#[component]
fn Users() -> Element {
    rsx! {
        Shell { active: "users",
            crate::views::users::RenderUsers {}
        }
    }
}

#[component]
fn SettingsSection(section: String) -> Element {
    // The section is read off the route inside `RenderSettings`, so this only has to exist as a route
    // target — declaring the prop is what stops the router stripping it.
    let _ = section;

    rsx! {
        Shell { active: "settings",
            crate::views::settings::RenderSettings {}
        }
    }
}

#[component]
fn Settings() -> Element {
    rsx! {
        Shell { active: "settings",
            crate::views::settings::RenderSettings {}
        }
    }
}

/// Not wrapped in `Shell`: it runs before there is a session, and asking `/me` first would only fail.
#[component]
fn AuthCallback(code: String, state: String, error: String) -> Element {
    rsx! {
        crate::views::auth_callback::RenderAuthCallback { code, state, error }
    }
}

#[component]
fn Logout() -> Element {
    rsx! {
        crate::views::logout::RenderLogout {}
    }
}

/// Open the WebSocket, once, and hold it for the session.
///
/// Called by Home when its projects have arrived rather than by the shell on the way past. Two reasons: the
/// shell had to write `ws_started` during render to guard against a second socket, which is the signal write
/// in the render body that §16 of the design patterns forbids; and until a project is known there is nothing
/// to subscribe to, so starting earlier only opened a socket that had nothing to say.
///
/// Idempotent: the `ws_started` flag is checked and set inside, so calling it on every load of the board is
/// safe.
pub fn start_ws() {
    let mut app_state = consume_context::<Signal<AppState>>();

    if app_state.peek().ws_started {
        return;
    }

    app_state.write().ws_started = true;
    kick_off_ws(app_state);
}

/// Hold the socket open and bump `board_revision` when the server says a board changed.
///
/// It carries nothing: the session is a cookie, which the browser attaches to the handshake by itself.
///
/// No reconnect loop yet: a dropped socket leaves the board static until the page is reloaded, and since the
/// live dot came off the header there is nothing on screen that says so — only a line in the console. If that
/// starts biting, the fix is a reconnect, not putting the dot back.
fn kick_off_ws(mut app_state: Signal<AppState>) {
    spawn(async move {
        // Bound before use: `get_origin` borrows from the settings, so calling it on a temporary would
        // not outlive the statement.
        let settings = dioxus_utils::js::GlobalAppSettings::new();
        let origin = settings.get_origin();

        let ws_origin = if origin.starts_with("https") {
            origin.replacen("https", "wss", 1)
        } else {
            origin.replacen("http", "ws", 1)
        };

        // No token in the url any more: the session is a cookie, and the browser sends cookies on the
        // WebSocket handshake — which is the one thing the WebSocket API will do that it will not do with a
        // header. It used to be spelled here, where it landed in browser history and in proxy logs.
        let ws_url = if ws_origin.ends_with('/') {
            format!("{ws_origin}ws")
        } else {
            format!("{ws_origin}/ws")
        };

        // The channel is installed before the socket opens, so a `watch` sent by Home while the
        // handshake is still in flight waits in it rather than being dropped.
        let mut outgoing = crate::web::install_ws_sender();

        match WebSocket::open(&ws_url) {
            Ok(ws) => {
                // Split so sending and receiving are independent: the read half blocks on the server, and
                // a `watch` must not have to wait behind it.
                let (mut write, mut read) = ws.split();

                spawn(async move {
                    while let Some(payload) = outgoing.next().await {
                        if write.send(Message::Text(payload)).await.is_err() {
                            break;
                        }
                    }
                });

                while let Some(message) = read.next().await {
                    match message {
                        Ok(Message::Text(text)) => match ServerWsMessage::parse(&text) {
                            // Landed as-is for the view to apply. Nothing is requested and nothing is
                            // emptied first, which is the whole point: the screen does not flinch.
                            ServerWsMessage::BoardSnapshot(snapshot) => {
                                app_state.write().board_pushed(snapshot);
                            }
                            // No board came with it, so the view re-reads — the old protocol, kept because
                            // it is the one thing that works when the server cannot build a snapshot.
                            ServerWsMessage::ProjectChanged => {
                                app_state.write().board_invalidated();
                            }
                            ServerWsMessage::Error(err) => {
                                crate::web::console_log(format!("ws: {err}").as_str());
                            }
                            ServerWsMessage::Unknown(raw) => {
                                crate::web::console_log(format!("ws: unknown {raw}").as_str());
                            }
                        },
                        Ok(Message::Bytes(_)) => {}
                        Err(err) => {
                            crate::web::console_log(format!("ws error: {err:?}").as_str());
                            break;
                        }
                    }
                }

                // The socket is gone. Nothing on screen says so — see `kick_off_ws`.
                crate::web::console_log("ws closed");
            }
            Err(err) => {
                crate::web::console_log(format!("cannot open ws: {err:?}").as_str());
            }
        }
    });
}
