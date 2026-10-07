use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::users::UserResponse;

/// The roster.
///
/// Being here is what allows a sign-in at all; it grants no project on its own — that is set per project
/// in Projects setup. An email is the identity and is never edited: every task assignment and every
/// comment author points at it, so a changed address is a new person rather than a rename.
#[component]
pub fn RenderUsers() -> Element {
    let mut cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    let roster = match get_roster(cs, &cs_ra) {
        Ok(roster) => roster,
        Err(element) => return element,
    };

    // Not an admin. The endpoint answers that rather than failing, so it is a shape of the data and not
    // an error banner.
    let Some(users) = roster else {
        return rsx! {
            div { class: "page-header",
                h1 { class: "page-title", "Users" }
            }
            div { class: "empty-note", "Admins only." }
        };
    };

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Users" }
        }

        // Saving anything re-reads. `reset()` puts the `DataState` back to `None` and the next render
        // loads — the server is the only thing that says what was actually stored.
        RenderCreateUser { on_saved: move |_| cs.write().roster.reset() }

        div { class: "card",
            div { class: "card-title", "Roster" }
            if users.is_empty() {
                div { class: "empty-note", "Nobody yet." }
            } else {
                div { class: "table-responsive",
                    table { class: "table",
                        thead {
                            tr {
                                th { "Email" }
                                th { "Name" }
                                th { "Admin" }
                                th { "Disabled" }
                                th { "" }
                            }
                        }
                        tbody {
                            for user in users.iter() {
                                RenderUserRow {
                                    key: "{user.email}",
                                    user: user.clone(),
                                    on_saved: move |_| cs.write().roster.reset(),
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Everything this screen holds, in one struct behind one signal.
///
/// `Option` inside the `DataState` because "not an admin" is an answer the endpoint gives on purpose —
/// see [`get_roster`].
#[derive(Default)]
struct ComponentState {
    roster: DataState<Option<Vec<UserResponse>>>,
}

/// The roster, loading it on first render.
///
/// `Err` carries what to show instead — the spinner or the failure — so the caller stays a straight line.
/// `Ok(None)` from the endpoint is carried through as loaded data rather than folded into an error: an
/// admin-only screen saying "Admins only" is working correctly, not failing.
fn get_roster(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<Option<&[UserResponse]>, Element> {
    match cs_ra.roster.as_ref() {
        RenderState::None => {
            spawn(async move {
                cs.write().roster.set_loading();

                match crate::api::get_users().await {
                    Ok(response) => cs.write().roster.set_loaded(response.map(|itm| itm.users)),
                    Err(err) => cs.write().roster.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(roster) => Ok(roster.as_deref()),
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
            h1 { class: "page-title", "Users" }
        }
        div { class: "error-banner", "{message}" }
    }
}

/// The add-someone form. A write, so no `DataState` — what it holds is what is being typed and how the
/// last submit went.
#[derive(Default)]
struct CreateUserState {
    email: String,
    name: String,
    admin: bool,
    error: String,
    saving: bool,
}

impl CreateUserState {
    fn can_save(&self) -> bool {
        self.email.contains('@') && !self.saving
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
fn RenderCreateUser(on_saved: EventHandler<()>) -> Element {
    let mut cs = use_signal(CreateUserState::default);
    let cs_ra = cs.read();

    let submit = move |_| {
        let (email, name, admin) = {
            let ra = cs.read();
            (ra.email.clone(), ra.name.clone(), ra.admin)
        };

        cs.write().begin_save();

        spawn(async move {
            match crate::api::create_user(&email, &name, admin).await {
                // Emptied back to a blank form: the row that was just added is now in the table below,
                // and leaving it in the inputs invites adding it twice.
                Ok(()) => {
                    cs.set(CreateUserState::default());
                    on_saved.call(());
                }
                Err(err) => cs.write().fail(err.message),
            }
        });
    };

    let email = cs_ra.email.as_str();
    let name = cs_ra.name.as_str();
    let admin = cs_ra.admin;
    let error = cs_ra.error.as_str();
    let can_save = cs_ra.can_save();

    rsx! {
        div { class: "card",
            div { class: "card-title", "Add someone" }
            if !error.is_empty() {
                div { class: "error-banner", "{error}" }
            }
            div { class: "form-row-inline",
                div { class: "form-row", style: "flex: 1 1 240px",
                    label { "Google account email" }
                    input {
                        r#type: "text",
                        value: "{email}",
                        oninput: move |event| cs.write().email = event.value().to_lowercase(),
                    }
                }
                div { class: "form-row", style: "flex: 1 1 180px",
                    label { "Name shown on the board" }
                    input {
                        r#type: "text",
                        value: "{name}",
                        oninput: move |event| cs.write().name = event.value(),
                    }
                }
                div { class: "checkbox-row",
                    input {
                        r#type: "checkbox",
                        id: "new-user-admin",
                        checked: admin,
                        onchange: move |event| cs.write().admin = event.checked(),
                    }
                    label { r#for: "new-user-admin", "Admin" }
                }
                button {
                    class: "btn btn-primary",
                    disabled: !can_save,
                    onclick: submit,
                    "Add"
                }
            }
            div { class: "field-hint",
                "It must be the Google account they will sign in with. An admin sees every project and may configure the product."
            }
        }
    }
}

/// One editable row. Also a write, so also no `DataState`.
struct UserRowState {
    name: String,
    admin: bool,
    disabled: bool,
    saving: bool,
}

impl UserRowState {
    fn new(user: &UserResponse) -> Self {
        Self {
            name: user.name.clone(),
            admin: user.admin,
            disabled: user.disabled,
            saving: false,
        }
    }
}

#[component]
fn RenderUserRow(user: UserResponse, on_saved: EventHandler<()>) -> Element {
    let mut cs = use_signal(|| UserRowState::new(&user));
    let cs_ra = cs.read();

    let email = user.email.clone();

    let save = move |_| {
        let email = email.clone();

        let (name, admin, disabled) = {
            let ra = cs.read();
            (ra.name.clone(), ra.admin, ra.disabled)
        };

        cs.write().saving = true;

        spawn(async move {
            let _ = crate::api::update_user(&email, &name, admin, disabled).await;

            cs.write().saving = false;
            on_saved.call(());
        });
    };

    // An admin by settings has no checkbox to untick — showing one that silently does nothing would be
    // worse than showing none, so it reads as granted-elsewhere instead.
    let admin_from_settings = user.admin_from_settings;

    let name = cs_ra.name.as_str();
    let admin = cs_ra.admin;
    let disabled = cs_ra.disabled;
    let saving = cs_ra.saving;

    rsx! {
        tr {
            td { class: "mono", "{user.email}" }
            td {
                input {
                    r#type: "text",
                    value: "{name}",
                    oninput: move |event| cs.write().name = event.value(),
                }
            }
            td {
                if admin_from_settings {
                    span { class: "tag", title: "Granted by the service settings, not by this row", "from settings" }
                } else {
                    input {
                        r#type: "checkbox",
                        checked: admin,
                        onchange: move |event| cs.write().admin = event.checked(),
                    }
                }
            }
            td {
                input {
                    r#type: "checkbox",
                    checked: disabled,
                    onchange: move |event| cs.write().disabled = event.checked(),
                }
            }
            td {
                button {
                    class: "btn btn-sm",
                    disabled: saving,
                    onclick: save,
                    "Save"
                }
            }
        }
    }
}
