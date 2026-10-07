use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};

use crate::AppRoute;

/// Where Google drops the browser back.
///
/// Exchanges the code for a session token, stores it and goes to Home. The URL is replaced on the way so
/// a reload does not re-submit a spent code — Google refuses it the second time, and the person would be
/// looking at a failure that is not one.
///
/// `code`, `state` and `error` arrive as route props rather than being read off `window.location`. The
/// first version read the location, which the router had already stripped, so every sign-in reported
/// "Google did not return a sign-in" — see the route declaration in `main.rs`.
#[component]
pub fn RenderAuthCallback(code: String, state: String, error: String) -> Element {
    let cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    // Nothing is rendered out of the exchange — it ends in a redirect — so the only branch that draws
    // anything is the failing one.
    let Err(element) = exchange_the_code(cs, &cs_ra, &code, &state, &error) else {
        return render_signing_in();
    };

    element
}

/// One struct, one signal. The exchange is a request whose whole visible result is which of the two
/// screens below is drawn, so it is a `DataState` like any other — `Loaded(())` only ever flashes past
/// on the way to Home.
#[derive(Default)]
struct ComponentState {
    exchange: DataState<()>,
}

/// Trade the code for a session token, once.
///
/// `Ok(())` means there is nothing to draw beyond "Signing in…" — the navigation has already been asked
/// for. `Err` carries the screen to show instead.
fn exchange_the_code(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
    code: &str,
    state: &str,
    error: &str,
) -> Result<(), Element> {
    match cs_ra.exchange.as_ref() {
        RenderState::None => {
            // Taken verbatim. The router has already percent-decoded the query by the time these reach
            // a prop, so there is nothing left here to undo — see `callback_value`.
            let code = callback_value(code);
            let state = callback_value(state);
            let error = callback_value(error);

            spawn(async move {
                cs.write().exchange.set_loading();

                // Google's own refusal, and the ordinary case of it is somebody pressing Cancel on the
                // consent screen. Reported as itself rather than as a fault: there is nothing to retry
                // differently, and "something went wrong" would be a lie about a deliberate choice.
                if !error.is_empty() {
                    let message = if error == "access_denied" {
                        "Sign-in was cancelled.".to_string()
                    } else {
                        format!("Google refused the sign-in: {error}")
                    };

                    cs.write().exchange.set_error(message);
                    return;
                }

                if code.is_empty() || state.is_empty() {
                    cs.write()
                        .exchange
                        .set_error("Google did not return a sign-in to complete.".to_string());
                    return;
                }

                match crate::api::finish_google_login(&code, &state).await {
                    Ok(response) => {
                        // Nothing to store: the session arrived as an `HttpOnly` cookie set on this very
                        // response, and the browser has already kept it. The token in the body is there for
                        // clients that are not browsers.
                        //
                        // What IS done here is clearing whatever an older build left in local storage, so a
                        // dead token does not sit there for a year. `response.token` is deliberately unused.
                        let _ = &response.token;
                        crate::web::storage::clear_session_token();
                        replace_url_with_root();
                        navigator().push(AppRoute::Home { search: None });
                        cs.write().exchange.set_loaded(());
                    }
                    Err(err) => cs.write().exchange.set_error(err.message),
                }
            });

            Ok(())
        }
        RenderState::Loading | RenderState::Loaded(()) => Ok(()),
        RenderState::Error(err) => Err(render_failure(err)),
    }
}

fn render_signing_in() -> Element {
    rsx! {
        div { class: "full-screen",
            div { class: "full-screen-form",
                div { class: "loading-note", "Signing in…" }
            }
        }
    }
}

fn render_failure(message: &str) -> Element {
    rsx! {
        div { class: "full-screen",
            div { class: "full-screen-form",
                h1 { class: "login-title", "Could not sign in" }
                div { class: "error-banner", "{message}" }
                button {
                    class: "btn btn-primary btn-google",
                    onclick: move |_| {
                        replace_url_with_root();
                        navigator().push(AppRoute::Home { search: None });
                    },
                    "Try again"
                }
            }
        }
    }
}

/// An OAuth callback parameter, exactly as it arrived.
///
/// Trimmed and otherwise untouched, and the *untouched* is the whole point. This decoded before, on the
/// belief that the router hands over the raw query slice. It does not — by the time a query segment is a
/// route prop the router has already percent-decoded it, so decoding again was one layer too many:
/// `%2B` came out of the router as `+`, and the second pass — reading `+` as a space, which is the right
/// rule for a form-encoded query and the wrong one for a value already decoded — turned it into a space.
///
/// That destroyed the `state`, which is base64 and carries a `+` about six times in ten. It surfaced as
/// a 401 that cleared on a retry, i.e. as something intermittent and server-side, which is what made it
/// worth three attempts to find. What it actually looked like, in the request body:
///
/// ```text
/// "state":"xUBhAVFTuzudAnTp oXf7jW1/S…"
///                          ^ was a +
/// ```
fn callback_value(src: &str) -> String {
    src.trim().to_string()
}

/// Drop the query string without reloading, so the spent code is not left sitting in the address bar.
fn replace_url_with_root() {
    if let Some(window) = web_sys::window()
        && let Ok(history) = window.history()
    {
        let _ = history.replace_state_with_url(&wasm_state(), "", Some("/"));
    }
}

fn wasm_state() -> js_sys::JsString {
    js_sys::JsString::from("")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The router hands these over ALREADY decoded, so this must not touch them.
    ///
    /// The previous version of these tests asserted the opposite — that `%2F` became `/` here — which is
    /// exactly the extra layer that destroyed the state. Kept as a test rather than deleted: it is the one
    /// assertion that would catch somebody adding the decode back.
    #[test]
    fn a_callback_value_is_passed_through_untouched() {
        assert_eq!(
            callback_value("xUBhAVFTuzudAnTp+oXf7jW1/S"),
            "xUBhAVFTuzudAnTp+oXf7jW1/S",
            "a + and a / are what a decoded base64 state looks like — both must survive"
        );

        assert_eq!(
            callback_value("4/0AXEQxICCHfSoVEzGIlLkSCcJ"),
            "4/0AXEQxICCHfSoVEzGIlLkSCcJ"
        );

        // A percent sequence that arrives here is a literal, not something to decode: the router is done.
        assert_eq!(callback_value("a%2Bb"), "a%2Bb");
    }

    /// Only the trimming survives, so an absent parameter still reads as absent.
    #[test]
    fn nothing_reads_as_nothing() {
        assert!(callback_value("").is_empty());
        assert!(callback_value("   ").is_empty());
    }
}
