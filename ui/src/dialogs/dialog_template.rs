use dioxus::prelude::*;

use super::DialogState;

/// The wrapper every dialog sits in.
///
/// Cancel and the close cross are built in — never add them in a dialog's own body, or a dialog ends up
/// with two ways to close that can drift apart.
pub fn dialog_template(title: &str, content: Element, btn_success: Element) -> Element {
    dialog_template_ex(title, content, btn_success, None)
}

/// [`dialog_template`] with a width class — `modal-lg` or `modal-xl`.
pub fn dialog_template_ex(
    title: &str,
    content: Element,
    btn_success: Element,
    extra_class: Option<&str>,
) -> Element {
    let footer = rsx! {
        div { class: "modal-footer",
            button { class: "btn", onclick: move |_| close(), "Cancel" }
            {btn_success}
        }
    };

    render_modal(title, content, Some(footer), extra_class, None)
}

/// A dialog with no footer at all — for one that only shows something.
///
/// Cancel is a way out of an edit, and a dialog with nothing to save has no edit to get out of: it left the
/// reader looking for the difference between Cancel and the cross, of which there is none. The cross in the
/// header is the way out.
pub fn dialog_template_read_only(
    title: &str,
    content: Element,
    extra_class: Option<&str>,
) -> Element {
    render_modal(title, content, None, extra_class, None)
}

/// A read-only dialog that came from ANOTHER dialog, with the way back in its header.
///
/// **The back arrow is not a second close.** Following a document reference off a task opens the document
/// here, and the reader has not left the task — the arrow puts it back exactly as it was, where the cross
/// still closes the lot. Two different exits, which is why the arrow is in the header rather than the
/// footer: a footer button beside Cancel would read as one of a pair of ways out of an edit, and there is
/// no edit.
pub fn dialog_template_with_back(
    title: &str,
    content: Element,
    on_back: EventHandler<()>,
    extra_class: Option<&str>,
) -> Element {
    render_modal(title, content, None, extra_class, Some(on_back))
}

fn render_modal(
    title: &str,
    content: Element,
    footer: Option<Element>,
    extra_class: Option<&str>,
    on_back: Option<EventHandler<()>>,
) -> Element {
    let modal_class = match extra_class {
        Some(extra) => format!("modal {extra}"),
        None => "modal".to_string(),
    };

    let title = title.to_string();

    rsx! {
        // **The backdrop does not close the dialog, deliberately.** It used to, and what that cost was
        // never a click on the grey: it was a drag that began on a card's text and ENDED out there, a
        // mis-hit beside a field, a click on a dialog that had just repainted — each of which threw away
        // what was open with no way back. Reading a long thread is what these dialogs are mostly for, and
        // the way out is the cross, which is a thing you have to mean to press.
        div {
            class: "modal-backdrop",
            div {
                class: "{modal_class}",
                div { class: "modal-header",
                    if let Some(on_back) = on_back {
                        button {
                            class: "modal-back",
                            title: "Back",
                            onclick: move |_| on_back.call(()),
                            "←"
                        }
                    }
                    div { class: "modal-title", "{title}" }
                    button { class: "modal-close", title: "Close", onclick: move |_| close(), "×" }
                }
                div { class: "modal-body", {content} }
                if let Some(footer) = footer {
                    {footer}
                }
            }
        }
    }
}

/// Closing is setting the state back to `None`, from anywhere — no dialog owns its own visibility.
pub fn close() {
    consume_context::<Signal<DialogState>>().set(DialogState::None);
}
