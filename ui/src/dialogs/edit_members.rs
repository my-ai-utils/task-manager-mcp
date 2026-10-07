use std::rc::Rc;

use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::users::UserResponse;

/// Who may see one project.
///
/// The roster is loaded here rather than passed in: this dialog is the only thing that needs it, and a
/// screen that carries data purely to hand it to a dialog goes stale the moment somebody edits the
/// roster in another tab.
#[derive(Default)]
struct ComponentState {
    roster: DataState<Vec<UserResponse>>,
    chosen: Vec<String>,
    error: String,
    saving: bool,
}

impl ComponentState {
    fn new(chosen: Vec<String>) -> Self {
        Self {
            chosen,
            ..Default::default()
        }
    }

    fn toggle(&mut self, email: &str, on: bool) {
        if on {
            if !self.chosen.iter().any(|itm| itm == email) {
                self.chosen.push(email.to_string());
            }
        } else {
            self.chosen.retain(|itm| itm != email);
        }
    }

    fn begin_save(&mut self) {
        self.saving = true;
        self.error = String::new();
    }

    fn fail(&mut self, message: String) {
        self.saving = false;
        self.error = message;
    }
}

#[component]
pub fn EditMembersDialog(project: Rc<ProjectResponse>, on_saved: EventHandler<()>) -> Element {
    let mut cs = use_signal(|| ComponentState::new(project.members.clone()));
    let cs_ra = cs.read();

    // By PREFIX, which is the only name a project has on this side — see `ProjectResponse`.
    let prefix = project.prefix.clone();

    let submit = move |_| {
        let prefix = prefix.clone();
        let members = cs.read().chosen.clone();

        cs.write().begin_save();

        spawn(async move {
            match crate::api::set_members(&prefix, members).await {
                Ok(()) => {
                    on_saved.call(());
                    super::close();
                }
                Err(err) => cs.write().fail(err.message),
            }
        });
    };

    let title = format!("Members · {}", project.prefix);
    let error = cs_ra.error.as_str();
    let saving = cs_ra.saving;

    let roster = match get_roster(cs, &cs_ra) {
        Ok(roster) => roster,
        Err(element) => {
            return super::dialog_template(&title, element, rsx! {});
        }
    };

    let chosen = cs_ra.chosen.clone();

    let content = rsx! {
        div { class: "field-hint",
            "Admins see every project without being listed here. Somebody on no project at all sees an empty Home rather than being locked out."
        }
        if !error.is_empty() {
            div { class: "error-banner", style: "margin-top: 10px", "{error}" }
        }

        if roster.is_empty() {
            div { class: "empty-note", style: "margin-top: 12px",
                "Nobody is on the roster yet — add people on the Users screen first."
            }
        } else {
            div { style: "margin-top: 12px",
                for user in roster.iter() {
                    div { class: "checkbox-row", key: "{user.email}",
                        input {
                            r#type: "checkbox",
                            id: "member-{user.email}",
                            checked: chosen.iter().any(|itm| itm == &user.email),
                            onchange: {
                                let email = user.email.clone();
                                move |event: Event<FormData>| {
                                    cs.write().toggle(&email, event.checked());
                                }
                            },
                        }
                        label { r#for: "member-{user.email}",
                            if user.name.trim().is_empty() {
                                "{user.email}"
                            } else {
                                "{user.name} · {user.email}"
                            }
                            if user.disabled {
                                span { class: "tag", style: "margin-left: 6px", "disabled" }
                            }
                        }
                    }
                }
            }
        }
    };

    let ok_button = rsx! {
        button { class: "btn btn-primary", disabled: saving, onclick: submit, "Save members" }
    };

    super::dialog_template(&title, content, ok_button)
}

/// The roster, loading it on first render.
///
/// `Err` carries what to show instead — loading or the failure — so the caller stays a straight line.
fn get_roster(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[UserResponse], Element> {
    match cs_ra.roster.as_ref() {
        RenderState::None => {
            spawn(async move {
                cs.write().roster.set_loading();

                match crate::api::get_users().await {
                    // An admin-only endpoint answering "not for you" is `Ok(None)`. Only an admin can
                    // reach this dialog, so treat it as an empty roster rather than inventing an error.
                    Ok(response) => cs
                        .write()
                        .roster
                        .set_loaded(response.map(|itm| itm.users).unwrap_or_default()),
                    Err(err) => cs.write().roster.set_error(err.message),
                }
            });

            Err(rsx! {
                div { class: "loading-note", "Loading the roster…" }
            })
        }
        RenderState::Loading => Err(rsx! {
            div { class: "loading-note", "Loading the roster…" }
        }),
        RenderState::Loaded(users) => Ok(users.as_slice()),
        RenderState::Error(err) => Err(rsx! {
            div { class: "error-banner", "{err}" }
        }),
    }
}
