use dioxus::prelude::*;
use task_manager_shared::documents::{is_zip_upload, render_size};

/// What an import needs, once the reader has chosen the file.
#[derive(Clone, PartialEq)]
pub struct ImportSubmit {
    /// Which project it is poured into, by prefix.
    pub project: String,
    pub bytes: Vec<u8>,
}

#[derive(Default)]
struct ComponentState {
    file: Option<PickedFile>,
    reading: bool,
    /// The reader having ticked the box that says they understand what this does. Off by default, and the
    /// button is dead until it is on — see the note on the component.
    confirmed: bool,
}

#[derive(Clone, PartialEq)]
struct PickedFile {
    name: String,
    bytes: Vec<u8>,
    is_zip: bool,
}

/// Pour an exported project into this one.
///
/// **The one dialog in this product that can change a board wholesale**, and the shape of it is built around
/// that: it says what will happen in plain words before anything is picked, and the button stays dead behind
/// a checkbox. Not ceremony — there is no undo. An import adds every goal and task in the file and REPLACES
/// this project's settings with the file's, and somebody who meant to press Export on the row above has to
/// find that out before it happens rather than after.
///
/// What it does not do is validate the archive. That is the server's job and it does it properly — the format
/// marker, the files in it, every path — and doing half of it here would mean two answers to one question, of
/// which one is always the wrong one. All that is checked here is that a file was chosen, plus the name-based
/// zip test that is only there to catch the obvious slip early.
#[component]
pub fn ImportProjectDialog(
    project: String,
    project_name: String,
    on_submit: EventHandler<ImportSubmit>,
) -> Element {
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
                label { "Into" }
                div { "{project_name} ({project})" }
                div { class: "field-hint",
                    "Goals and tasks are ADDED — nothing already on this board is removed. They take fresh ids out of this project's counter, so the ids in the file are not the ids they get."
                }
            }

            div { class: "form-row",
                label { "Replaces" }
                div { class: "field-hint",
                    "This project's name, description, archive window and its column and task-type templates are replaced with the ones in the file. A template that is not on this instance, or a prefix another project holds, is left alone and reported afterwards."
                }
            }

            div { class: "form-row",
                label { "File" }
                input {
                    r#type: "file",
                    accept: ".zip",
                    onchange: move |event| {
                        let Some(file) = event.files().into_iter().next() else {
                            return;
                        };

                        let name = file.name();
                        let is_zip = is_zip_upload(
                            &name,
                            file.content_type().filter(|itm| !itm.trim().is_empty()).as_deref(),
                        );

                        {
                            let mut write = cs.write();
                            write.reading = true;
                            // A previous pick's confirmation must not carry into this one: agreeing to import
                            // one file is not agreeing to import the next.
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
                                        is_zip,
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

                    if !picked.is_zip {
                        div { class: "error-note",
                            "That does not look like a zip. An export is the file the Export button downloads."
                        }
                    }
                }
            }

            if picked.is_some() {
                div { class: "form-row",
                    div { class: "checkbox-row",
                        input {
                            r#type: "checkbox",
                            id: "import-confirm",
                            checked: confirmed,
                            onchange: move |event: Event<FormData>| {
                                cs.write().confirmed = event.checked();
                            },
                        }
                        label { r#for: "import-confirm",
                            "I understand this cannot be undone"
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

                on_submit.call(ImportSubmit {
                    project: project.clone(),
                    bytes: file.bytes,
                });
            },
            if feedback.saving { "Importing…" } else { "Import" }
        }
    };

    super::dialog_template("Import a project", content, ok)
}
