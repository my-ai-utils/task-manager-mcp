use dioxus::prelude::*;
use dioxus_utils::{DataState, RenderState};
use task_manager_shared::documents::{
    DocumentReference, FindDocumentResponse, parse_document_reference,
};

use super::DialogState;

/// The block a task or a goal draws for the documents it references.
///
/// **What it has is references, and nothing else.** The board snapshot carries the reference list and not the
/// documents, so this cannot show a size or a version without a request — and it deliberately does not make
/// one per card.
///
/// What a reference DOES carry is enough to draw a useful row, which is the difference a url made: a file in
/// a connected repository is drawn by its file name, with its path as the tooltip, because the reference
/// spells both out. A document of the project's own is still drawn by its id — its path is a row in a table
/// this screen has not fetched.
///
/// **A click opens the document IN this dialog rather than leaving for the Documents screen.** The reader is
/// in the middle of a task, and the document is the specification that task is done against: sending them to
/// another screen to read it means coming back to find the task, which is the trip nobody makes twice. The
/// header grows a back arrow that puts the task back exactly as it was.
#[component]
pub fn DocumentRefs(project: String, ids: Vec<String>) -> Element {
    rsx! {
        div { class: "task-view-attr",
            div { class: "task-view-attr-label", "Documents" }
            div { class: "doc-refs",
                for reference in ids.iter() {
                    {
                        let reference = reference.clone();
                        let project = project.clone();
                        let drawn = DrawnRef::of(&reference, &project);

                        rsx! {
                            button {
                                class: "doc-ref",
                                key: "{reference}",
                                title: "{drawn.title}",
                                onclick: move |_| {
                                    // Where to come back to is read HERE rather than remembered by the
                                    // document dialog: this is the one moment the dialog being left is
                                    // still the one that is open, and capturing it is what makes the back
                                    // arrow put the reader on the same task rather than on a fresh read
                                    // of it.
                                    let back = consume_context::<Signal<DialogState>>().read().clone();

                                    super::open(DialogState::ViewDocument {
                                        project: project.clone(),
                                        reference: reference.clone(),
                                        back: Box::new(back),
                                    });
                                },
                                span { class: "doc-ref-icon", "{drawn.icon}" }
                                span { class: "doc-ref-id", "{drawn.label}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One reference as a row: what to show, and what to say about it on hover.
struct DrawnRef {
    icon: &'static str,
    label: String,
    title: String,
}

impl DrawnRef {
    /// **The two kinds are marked differently on purpose.** A file in a connected repository is somebody
    /// else's — it has no history here, and editing it writes into a working copy rather than into this
    /// board — and a reader who cannot tell the two apart on a card learns the difference at the worst
    /// possible moment. The mark is the same one the Documents tree uses for the same thing.
    fn of(reference: &str, project: &str) -> Self {
        match parse_document_reference(reference) {
            Some(DocumentReference::Mirror { project, path }) => Self {
                icon: "🐙",
                label: task_manager_shared::documents::document_file_name(&path).to_string(),
                title: format!("{path} — in a repository connected to {project}. Open it"),
            },
            Some(DocumentReference::Own { project, id }) => Self {
                icon: "📄",
                label: id,
                title: format!("A document of {project}. Open it"),
            },
            // A bare id, which is what every reference stored before references were urls still is. Drawn
            // by the id, since that is the whole of what it says — the board comes from the card it is on.
            None => Self {
                icon: "📄",
                label: reference.to_string(),
                title: format!("A document of {project}. Open it"),
            },
        }
    }
}

/// One document, read without leaving the task or the goal that pointed at it.
///
/// **The same rendering the Documents screen uses**, through `render_found` — a second one would be a second
/// set of decisions about framing, sandboxing and content types to keep in step. What differs is the frame
/// around it: a header with a way back instead of a tree.
///
/// `on_back` restores the dialog this was opened from, and the router owns it — the same rule every dialog
/// here follows, that a dialog reports what happened and something else decides what to do about it. What
/// it restores is the state carried on `DialogState::ViewDocument`, not a fresh read: re-opening the task by
/// id would be a request, and it would land on whatever that task looks like now rather than on what the
/// reader was reading.
#[component]
pub fn ViewDocumentDialog(
    project: String,
    reference: String,
    on_back: EventHandler<()>,
) -> Element {
    let mut cs = use_signal(DataState::<FindDocumentResponse>::default);
    let cs_ra = cs.read();

    // The board the REFERENCE names, falling back to the card's own for a bare id — which names no board
    // and can only mean the one it is written on.
    let project = parse_document_reference(&reference)
        .map(|itm| itm.project().to_string())
        .unwrap_or(project);

    let title = document_title(&reference);

    let body = match cs_ra.as_ref() {
        RenderState::None => {
            let project = project.clone();
            let reference = reference.clone();

            spawn(async move {
                cs.write().set_loading();

                // The reference goes over the wire as it stands: the server unwraps it wherever it takes
                // an id, which is what makes one worth writing down in the first place.
                match crate::api::get_document(&project, &reference).await {
                    Ok(found) => cs.write().set_loaded(found),
                    Err(err) => cs.write().set_error(err.message),
                }
            });

            rsx! {
                div { class: "viewer-note", "loading…" }
            }
        }
        RenderState::Loading => rsx! {
            div { class: "viewer-note", "loading…" }
        },
        // `show_source` is deliberately false and has no switch here. This dialog is for reading the
        // specification a piece of work is done against; the source toggle belongs with the tree, on the
        // screen for working through documents, and a second control in a modal over a board is one more
        // thing between the reader and the text.
        RenderState::Loaded(found) => crate::views::documents::render_found(&project, found, false),
        RenderState::Error(err) => crate::views::documents::render_viewer_note(err.as_str(), true),
    };

    let content = rsx! {
        div { class: "doc-dialog",
            {body}
            // A way out to the full screen, kept because a modal is the one place a PDF cannot have the
            // pane its viewer wants. It LEAVES, which is why it is a footnote under the document rather
            // than a button beside the back arrow: the arrow is the ordinary way out of here.
            div { class: "doc-dialog-foot",
                a {
                    class: "doc-dialog-open",
                    href: "/documents?selected={reference}",
                    onclick: move |_| super::close(),
                    "Open in Documents"
                }
            }
        }
    };

    // The size a task is read at, not a column in the middle of the window: this dialog IS the task dialog
    // for as long as the reader is on the document, and a width that changed under the back arrow would
    // make the window jump on every click.
    super::dialog_template_with_back(&title, content, on_back, Some("modal-document"))
}

/// What the header says: the document's path for a repository's file, and the id otherwise.
///
/// Read off the reference rather than out of the loaded document, so the title is right on the first frame
/// — a header that says "Document" and then changes to a path once the request lands reads as a glitch.
fn document_title(reference: &str) -> String {
    match parse_document_reference(reference) {
        Some(DocumentReference::Mirror { path, .. }) => path,
        Some(DocumentReference::Own { id, .. }) => id,
        None => reference.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The name is what a reader is looking for, and a reference to a repository's file carries one — which
    /// is the difference between a row that says `system.md` and a row that says a 25-character id.
    #[test]
    fn a_repositorys_file_is_drawn_by_its_name() {
        let drawn = DrawnRef::of("raw/TM/github/specs/design/system.md", "TM");

        assert_eq!(drawn.label, "system.md");
        assert_eq!(drawn.icon, "🐙");
        // The path is on the tooltip, because two repositories can both hold a `README.md`.
        assert!(
            drawn.title.contains("github/specs/design/system.md"),
            "the path is what tells two of them apart: {}",
            drawn.title
        );
    }

    /// A document of the project's own has no name to draw until somebody fetches it, so the id is still
    /// what the row shows — with or without the url around it.
    #[test]
    fn a_document_of_the_projects_own_is_drawn_by_its_id() {
        for spelling in [
            "raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8",
            // The legacy spelling, still on cards written before references were urls.
            "01K2C4Q0S1T2U3V4W5X6Y7Z8",
        ] {
            let drawn = DrawnRef::of(spelling, "TM");

            assert_eq!(drawn.label, "01K2C4Q0S1T2U3V4W5X6Y7Z8");
            assert_eq!(drawn.icon, "📄");
        }
    }

    /// The header is right on the FIRST frame, before anything has been fetched — a title that changes
    /// once the request lands reads as a glitch.
    #[test]
    fn the_header_names_the_document_before_it_is_read() {
        assert_eq!(
            document_title("raw/TM/github/specs/design/system.md"),
            "github/specs/design/system.md"
        );
        assert_eq!(
            document_title("raw/TM/document/01K2C4Q0S1T2U3V4W5X6Y7Z8"),
            "01K2C4Q0S1T2U3V4W5X6Y7Z8"
        );
    }
}
