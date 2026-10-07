use dioxus::prelude::*;

/// The only pre-auth screen: one button.
///
/// There is nothing to type — no password lives here, and the address is whatever Google says it is.
#[component]
pub fn RenderLogin() -> Element {
    let mut cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    let start_login = move |_| {
        cs.write().begin();

        spawn(async move {
            match crate::api::get_google_auth_url().await {
                // Nothing to store: the answer is somewhere to go, and the browser leaves this page.
                Ok(response) => crate::web::navigate_to(response.url.as_str()),
                Err(err) => cs.write().fail(err.message),
            }
        });
    };

    let error_text = cs_ra.error.as_str();
    let is_going = cs_ra.going;

    rsx! {
        div { class: "full-screen",
            div { class: "full-screen-form",
                h1 { class: "login-title", "Task Manager" }
                p { class: "login-note",
                    "The board is worked by agents and configured here. Sign in with the Google account an admin put on the roster."
                }

                if !error_text.is_empty() {
                    div { class: "error-banner", "{error_text}" }
                }

                button {
                    class: "btn btn-primary btn-google",
                    disabled: is_going,
                    onclick: start_login,
                    if is_going {
                        "Redirecting…"
                    } else {
                        "Sign in with Google"
                    }
                }
            }
        }
    }
}

/// One struct, one signal. No `DataState`: the request here is an action rather than something to show —
/// its success is that the browser leaves for Google, so there is no loaded value this screen ever draws.
#[derive(Default)]
struct ComponentState {
    going: bool,
    error: String,
}

impl ComponentState {
    fn begin(&mut self) {
        self.going = true;
        self.error = String::new();
    }

    fn fail(&mut self, message: String) {
        self.going = false;
        self.error = message;
    }
}
