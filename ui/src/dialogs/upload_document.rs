use dioxus::prelude::*;
use task_manager_shared::documents::{
    document_file_name, is_zip_upload, normalise_document_path, render_size,
};

/// What an upload needs, once the reader has chosen it.
///
/// Two shapes rather than one with a flag, because the two say different things: a document goes to a PATH the
/// reader chose, and an archive goes to a FOLDER while what each of its files is called is already decided —
/// by whoever made the zip. A single struct would have to carry a name that means nothing half the time.
#[derive(Clone, PartialEq)]
pub enum UploadSubmit {
    /// One file, at one path.
    Document {
        project: String,
        path: String,
        bytes: Vec<u8>,
        content_type: Option<String>,
    },
    /// A zip, unpacked into a folder — the archive itself is not stored.
    Archive {
        project: String,
        folder: String,
        bytes: Vec<u8>,
    },
}

#[derive(Default)]
struct ComponentState {
    /// The folder the document goes in. Typed, and a dropdown of the folders that exist writes into it — so
    /// choosing an existing one and creating a new one are the same control rather than two that disagree.
    folder: String,
    /// The name it gets. Seeded from the chosen file and then editable: uploading `report (3).pdf` under a
    /// sensible name is the common case, and renaming afterwards is an MCP call.
    ///
    /// Unused for an archive — the names in there are the names of the files in there.
    name: String,
    file: Option<PickedFile>,
    reading: bool,
    /// The reader overriding what a zip does: store it as one document rather than unpack it. Off by default,
    /// because a zip that somebody wanted to KEEP as a zip is the rare case — the common one is a folder of
    /// documents that had to be put in something to be carried here.
    keep_archive: bool,
}

#[derive(Clone, PartialEq)]
struct PickedFile {
    bytes: Vec<u8>,
    content_type: Option<String>,
    /// Whether this is a zip, worked out when it was picked — by name first, because the content type a
    /// browser reports for one is three different strings depending on which browser it is.
    is_zip: bool,
}

/// Upload a file into a project's documents.
///
/// **The one dialog in this product that writes something other than a colour**, and it exists because the
/// alternative is not a person using MCP — it is a person unable to upload at all: a PDF on a laptop cannot
/// reach an agent without being base64-ed by hand into a tool call.
///
/// Folders are not created here because folders do not exist: they are read off the paths of the documents in
/// them. So "make a folder" and "choose a folder" are one act — type a path — and the dropdown beside it is a
/// shortcut that fills the same box with one that is already in use.
///
/// **A ZIP is unpacked rather than stored**, and the dialog says so as soon as one is picked: the Name box
/// goes away — every file in there is already called something — and the button says Unpack. One archive is
/// how a folder of documents gets here, which no amount of one-file-at-a-time is.
///
/// **It opens on the folder the reader is in**, handed in as `initial_folder`. Seeded rather than bound: from
/// the first render the box is the reader's, and typing over it is not fighting a value that keeps coming
/// back.
#[component]
pub fn UploadDocumentDialog(
    project: String,
    initial_folder: String,
    folders: Vec<String>,
    on_submit: EventHandler<UploadSubmit>,
) -> Element {
    let mut cs = use_signal(|| ComponentState {
        folder: initial_folder.clone(),
        ..Default::default()
    });
    let cs_ra = cs.read();

    let feedback = super::feedback();

    let folder = cs_ra.folder.clone();
    let name = cs_ra.name.clone();
    let picked = cs_ra.file.clone();
    let reading = cs_ra.reading;
    let keep_archive = cs_ra.keep_archive;

    // A zip the reader has not overridden. Everything below branches on this one value: what is asked for,
    // what the button says, and which of the two calls the router makes.
    let unpacking = picked
        .as_ref()
        .map(|itm| itm.is_zip && !cs_ra.keep_archive)
        .unwrap_or(false);

    // What the document will actually be called, worked out the same way the server will work it out — so the
    // reader is shown the answer rather than the inputs to it. An archive has no single path: its entries keep
    // the names they have inside it, so only the folder is validated.
    let full_path = if unpacking {
        None
    } else {
        join_path(&folder, &name)
    };

    // Where an archive lands, in the same place the path of a single file is spelled out — a zip can write a
    // hundred documents, so "into which folder" is the one thing worth being sure of before pressing it.
    let destination = folder.trim().trim_matches('/');

    // The folder is checked HERE for an archive rather than being left to the server, because the server
    // checks it once per entry: a folder that is not a path would come back as a hundred identical skips.
    let path_problem = if unpacking {
        if destination.is_empty() {
            None
        } else {
            normalise_document_path(destination).err()
        }
    } else {
        match full_path.as_deref().map(normalise_document_path) {
            Some(Err(problem)) => Some(problem),
            _ => None,
        }
    };

    let unpack_note = if destination.is_empty() {
        "Will be unpacked at the top level, keeping the folders inside the archive".to_string()
    } else {
        format!("Will be unpacked into {destination}/, keeping the folders inside the archive")
    };

    let ready = picked.is_some()
        && !reading
        && path_problem.is_none()
        && (unpacking || full_path.is_some());

    let content = rsx! {
        div { class: "upload-form",
            div { class: "form-row",
                label { "Folder" }
                div { class: "upload-folder",
                    input {
                        r#type: "text",
                        placeholder: "docs/design — leave empty for the top level",
                        value: "{folder}",
                        oninput: move |event| cs.write().folder = event.value(),
                    }
                    // A shortcut into the box beside it, not a second source of truth: picking here types the
                    // folder, and it can then be edited into a new one.
                    if !folders.is_empty() {
                        select {
                            class: "upload-folder-pick",
                            onchange: move |event| cs.write().folder = event.value(),
                            option { value: "", selected: folder.is_empty(), "— existing folders —" }
                            for existing in folders.iter() {
                                // Marked when it is the folder in the box, so the dropdown agrees with the
                                // text beside it — including on the first render, where the box arrives
                                // already holding the folder the reader was in.
                                option {
                                    value: "{existing}",
                                    selected: existing == &folder,
                                    "{existing}"
                                }
                            }
                        }
                    }
                }
                div { class: "field-hint",
                    "Folders are not stored — a folder exists for as long as a document is in it."
                }
            }

            div { class: "form-row",
                label { "File" }
                input {
                    r#type: "file",
                    onchange: move |event| {
                        let Some(file) = event.files().into_iter().next() else {
                            return;
                        };

                        let file_name = file.name();
                        let content_type = file.content_type().filter(|itm| !itm.trim().is_empty());
                        let is_zip = is_zip_upload(&file_name, content_type.as_deref());

                        // The name is seeded here rather than when the bytes land, so the box fills the
                        // instant the file is chosen instead of after a large read.
                        {
                            let mut write = cs.write();
                            write.reading = true;
                            // A previous pick's override must not survive into this one: a reader who chose to
                            // keep one archive whole did not thereby choose it for the next.
                            write.keep_archive = false;

                            if write.name.trim().is_empty() {
                                write.name = document_file_name(&file_name).to_string();
                            }
                        }

                        spawn(async move {
                            match file.read_bytes().await {
                                Ok(bytes) => {
                                    let mut write = cs.write();
                                    write.reading = false;
                                    write.file = Some(PickedFile {
                                        bytes: bytes.to_vec(),
                                        content_type,
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
                        "{render_size(picked.bytes.len() as i64)}"
                        if let Some(content_type) = picked.content_type.as_ref() {
                            " · {content_type}"
                        }
                    }
                }
            }

            // A zip is the one pick that changes what this dialog is for, so it says so where the Name box
            // would otherwise be — and offers the way back out for the reader who meant to keep the archive.
            if let Some(true) = picked.as_ref().map(|itm| itm.is_zip) {
                div { class: "form-row",
                    label { "Archive" }
                    div { class: "field-hint",
                        if unpacking {
                            "This is a zip — it will be UNPACKED, one document per file inside it, keeping the folders it holds. The archive itself is not stored."
                        } else {
                            "The zip will be stored as one document, exactly as it is."
                        }
                    }
                    div { class: "checkbox-row",
                        input {
                            r#type: "checkbox",
                            id: "upload-keep-archive",
                            checked: keep_archive,
                            onchange: move |event: Event<FormData>| {
                                cs.write().keep_archive = event.checked();
                            },
                        }
                        label { r#for: "upload-keep-archive",
                            "Keep the archive as one file instead of unpacking it"
                        }
                    }
                }
            }

            if !unpacking {
                div { class: "form-row",
                    label { "Name" }
                    input {
                        r#type: "text",
                        placeholder: "system.md",
                        value: "{name}",
                        oninput: move |event| cs.write().name = event.value(),
                    }
                }
            }

            // Where it lands, said out loud. Uploading onto a path that is taken writes a new version of what
            // is there — which is the right behaviour and a surprise if nobody showed you the path first, and
            // an archive can do it to a hundred documents at once.
            if let Some(problem) = path_problem {
                div { class: "error-note", "{problem}" }
            } else if let Some(full_path) = full_path.as_ref() {
                div { class: "field-hint", "Will be stored as {full_path}" }
            } else if unpacking {
                div { class: "field-hint", "{unpack_note}" }
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

                let submit = if file.is_zip && !cs_ra.keep_archive {
                    UploadSubmit::Archive {
                        project: project.clone(),
                        folder: cs_ra.folder.clone(),
                        bytes: file.bytes,
                    }
                } else {
                    let Some(path) = join_path(&cs_ra.folder, &cs_ra.name) else {
                        return;
                    };

                    UploadSubmit::Document {
                        project: project.clone(),
                        path,
                        bytes: file.bytes,
                        content_type: file.content_type,
                    }
                };

                drop(cs_ra);

                on_submit.call(submit);
            },
            if feedback.saving {
                if unpacking { "Unpacking…" } else { "Uploading…" }
            } else if unpacking {
                "Unpack"
            } else {
                "Upload"
            }
        }
    };

    let title = if unpacking {
        "Unpack an archive"
    } else {
        "Upload a document"
    };

    super::dialog_template(title, content, ok)
}

/// A folder and a name into one path. `None` when there is no name — a document has to be called something.
///
/// The server normalises what this produces, so a trailing slash on the folder or a stray space is not an
/// error here: it is written the way it was typed and cleaned up in one place.
fn join_path(folder: &str, name: &str) -> Option<String> {
    let name = name.trim();

    if name.is_empty() {
        return None;
    }

    let folder = folder.trim().trim_matches('/');

    if folder.is_empty() {
        Some(name.to_string())
    } else {
        Some(format!("{folder}/{name}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_and_a_name_make_a_path() {
        assert_eq!(join_path("docs", "a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(join_path("", "a.md").as_deref(), Some("a.md"));
        assert_eq!(join_path("  ", " a.md ").as_deref(), Some("a.md"));

        // A slash the reader typed either way round is not an error — it is the same folder.
        assert_eq!(join_path("/docs/", "a.md").as_deref(), Some("docs/a.md"));
        assert_eq!(
            join_path("docs/design", "a.md").as_deref(),
            Some("docs/design/a.md")
        );
    }

    #[test]
    fn without_a_name_there_is_no_path() {
        assert_eq!(join_path("docs", ""), None);
        assert_eq!(join_path("docs", "   "), None);
    }
}
