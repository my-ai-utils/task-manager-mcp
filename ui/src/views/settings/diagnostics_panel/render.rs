use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::system::DiagnosticsResponse;

/// The Diagnostics section of Settings — read-only on purpose.
///
/// There is nothing to configure here: the Google credentials, the encryption key and the admin list all
/// live in the service settings rather than the database, because credentials entered through a screen
/// that itself requires authentication would leave a fresh deployment with no way in.
///
/// What it is for is the first question when a sign-in fails — which client id was picked up and which
/// redirect URI is expected. Reading that beats guessing it from logs.
#[component]
pub fn DiagnosticsPanel() -> Element {
    let cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    let current = match get_diagnostics(cs, &cs_ra) {
        Ok(current) => current,
        Err(element) => return element,
    };

    // Not an admin. The endpoint says so rather than failing, so it is a shape of the data.
    let Some(current) = current else {
        return rsx! {
            div { class: "page-header",
                h1 { class: "page-title", "Settings" }
            }
            div { class: "empty-note", "Admins only." }
        };
    };

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Settings" }
        }
        p { class: "page-note",
            "Read-only. These values come from the service settings, not from the database — that is what lets the service authenticate before anybody has signed in. To change one, change the settings template and redeploy."
        }

        div { class: "card",
            div { class: "card-title", "Google sign-in" }
            div { class: "table-responsive",
                table { class: "table",
                    tbody {
                        tr {
                            th { "Configured" }
                            td {
                                if current.google_configured {
                                    "Yes"
                                } else {
                                    "No — client id or secret is missing, and nobody can sign in"
                                }
                            }
                        }
                        tr {
                            th { "Client id" }
                            td { class: "mono", "{current.google_client_id}" }
                        }
                        tr {
                            th { "Redirect URI" }
                            td { class: "mono", "{current.google_redirect_uri}" }
                        }
                    }
                }
            }
            div { class: "field-hint",
                "The redirect URI must match the one registered in the Google console byte for byte. A mismatch is the single most common cause of a sign-in that ends on a Google error page."
            }
        }

        div { class: "card",
            div { class: "card-title", "This deployment" }
            div { class: "table-responsive",
                table { class: "table",
                    tbody {
                        tr {
                            th { "Admins in settings" }
                            td { "{current.admins_in_settings}" }
                        }
                        tr {
                            th { "Projects" }
                            td { "{current.projects_amount}" }
                        }
                        tr {
                            th { "People on the roster" }
                            td { "{current.users_amount}" }
                        }
                        tr {
                            th { "Home tabs connected" }
                            td { "{current.connected_homes}" }
                        }
                    }
                }
            }
            div { class: "field-hint",
                "Admins in settings is the emergency door: on an empty database nobody is an admin in Postgres, and those addresses are what let the first person in. If it is zero and the roster is empty, nobody can get in at all."
            }
        }
    }
}

/// One struct, one signal — and the only thing this screen holds is what it read.
///
/// `Option` inside, because "not an admin" is what the endpoint answers rather than a failure.
#[derive(Default)]
struct ComponentState {
    data: DataState<Option<DiagnosticsResponse>>,
}

fn get_diagnostics(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<Option<&DiagnosticsResponse>, Element> {
    match cs_ra.data.as_ref() {
        RenderState::None => {
            spawn(async move {
                cs.write().data.set_loading();

                match crate::api::get_diagnostics().await {
                    Ok(response) => cs.write().data.set_loaded(response),
                    Err(err) => cs.write().data.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(data) => Ok(data.as_ref()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "loading-note", "Loading…" }
    }
}

fn render_error(message: &str) -> Element {
    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Settings" }
        }
        div { class: "error-banner", "{message}" }
    }
}
