use dioxus::prelude::*;
use task_manager_shared::documents::render_size;

#[derive(Default)]
struct ComponentState {
    file: Option<PickedFile>,
    reading: bool,
    /// The reader having ticked the box. Off by default and the button is dead until it is on — see the
    /// note on the component.
    confirmed: bool,
}

#[derive(Clone, PartialEq)]
struct PickedFile {
    name: String,
    bytes: Vec<u8>,
    looks_like_yaml: bool,
}

/// Apply a templates YAML file to this instance.
///
/// **It replaces configuration that projects are already following**, which is why it is shaped like the
/// project import rather than like an upload: it says what will happen before anything is picked, and the
/// button stays dead behind a checkbox. A template already here is replaced by the one in the file, and
/// every project following it changes shape in the same write.
///
/// Nothing is validated here. The server does it properly — the format marker, every id, every colour —
/// and half-doing it on this side would be two answers to one question, of which one is always the wrong
/// one. The extension check below is only there to catch the obvious slip early, and it does not block.
#[component]
pub fn ImportTemplatesDialog(on_submit: EventHandler<Vec<u8>>) -> Element {
    let mut cs = use_signal(ComponentState::default);
    let cs_ra = cs.read();

    let feedback = super::feedback();

    let picked = cs_ra.file.clone();
    let reading = cs_ra.reading;
    let confirmed = cs_ra.confirmed;

    let ready = picked.is_some() && !reading && confirmed;

    let content = rsx! {
        div { class: "upload-form",
            div { class: "form-row",
                label { "What this does" }
                div { class: "field-hint",
                    "Every template in the file is applied by its id. One that is already here is REPLACED — and every project following it changes shape in the same write. Nothing is deleted: a task sitting in a column the new version does not have keeps its status and reads as Todo until the column comes back."
                }
            }

            div { class: "form-row",
                label { "File" }
                input {
                    r#type: "file",
                    accept: ".yaml,.yml",
                    onchange: move |event| {
                        let Some(file) = event.files().into_iter().next() else {
                            return;
                        };

                        let name = file.name();
                        let lower = name.to_lowercase();
                        let looks_like_yaml =
                            lower.ends_with(".yaml") || lower.ends_with(".yml");

                        {
                            let mut write = cs.write();
                            write.reading = true;
                            // A previous pick's confirmation must not carry into this one: agreeing to
                            // apply one file is not agreeing to apply the next.
                            write.confirmed = false;
                            write.file = None;
                        }

                        spawn(async move {
                            match file.read_bytes().await {
                                Ok(bytes) => {
                                    let mut write = cs.write();
                                    write.reading = false;
                                    write.file = Some(PickedFile {
                                        name,
                                        bytes: bytes.to_vec(),
                                        looks_like_yaml,
                                    });
                                }
                                Err(_) => {
                                    let mut write = cs.write();
                                    write.reading = false;
                                    write.file = None;
                                }
                            }
                        });
                    },
                }

                if reading {
                    div { class: "field-hint", "Reading the file…" }
                } else if let Some(picked) = picked.as_ref() {
                    div { class: "field-hint",
                        "{picked.name} · {render_size(picked.bytes.len() as i64)}"
                    }

                    if !picked.looks_like_yaml {
                        div { class: "error-note",
                            "That does not look like a YAML file. A templates export is what the Export button downloads."
                        }
                    }
                }
            }

            if picked.is_some() {
                div { class: "form-row",
                    div { class: "checkbox-row",
                        input {
                            r#type: "checkbox",
                            id: "import-templates-confirm",
                            checked: confirmed,
                            onchange: move |event: Event<FormData>| {
                                cs.write().confirmed = event.checked();
                            },
                        }
                        label { r#for: "import-templates-confirm",
                            "I understand this replaces templates projects are following"
                        }
                    }
                }
            }

            if !feedback.error.is_empty() {
                div { class: "error-note", "{feedback.error}" }
            }
        }
    };

    let ok = rsx! {
        button {
            class: "btn btn-primary",
            disabled: !ready || feedback.saving,
            onclick: move |_| {
                let cs_ra = cs.read();

                let Some(file) = cs_ra.file.clone() else {
                    return;
                };

                drop(cs_ra);

                on_submit.call(file.bytes);
            },
            if feedback.saving { "Importing…" } else { "Import" }
        }
    };

    super::dialog_template("Import templates", content, ok)
}
