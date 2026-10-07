use dioxus::prelude::*;

/// Signing out is dropping the token: the session is carried inside it, so there is nothing on the
/// server to invalidate. The endpoint is still called, because that is where the fact is documented and
/// because it is the hook if revocation ever becomes real.
#[component]
pub fn RenderLogout() -> Element {
    use_future(move || async move {
        let _ = crate::api::logout().await;

        crate::web::storage::clear_session_token();

        // A full reload rather than a route push: it throws away every signal, including the WebSocket
        // task and the cached `/me`, which a push would leave running under the login screen.
        crate::web::navigate_to("/");
    });

    rsx! {
        div { class: "full-screen",
            div { class: "loading-note", "Signing out…" }
        }
    }
}
