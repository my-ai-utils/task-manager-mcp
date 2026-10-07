use dioxus::prelude::*;

/// What a card dropped into Done asks for: what was actually done.
///
/// The rule is the server's and it is not new — an MCP status change into `done` is refused without a
/// comment, for the reason the Done column exists at all: it is what makes a board worth reading months
/// later, and "moved to done" records nothing anybody can use. So the drag cannot simply be refused here
/// either; it has to ask, or the gesture would read as broken.
///
/// The author is deliberately absent from this form. Unlike MCP, this side has a session — the server signs
/// the comment with whoever dragged the card, which is one thing this door does better.
#[component]
pub fn LandTaskDialog(handle: String, on_submit: EventHandler<String>) -> Element {
    let mut text = use_signal(String::new);

    // The submit outcome belongs to the router, which makes the request — see `DialogFeedback`.
    let feedback = super::feedback();
    let error = feedback.error.clone();

    let written = text.read().trim().to_string();
    let can_save = !written.is_empty() && !feedback.saving;

    let title = format!("Landing {handle}");

    let content = rsx! {
        if !error.is_empty() {
            div { class: "error-banner", "{error}" }
        }

        div { class: "form-row",
            label { "What was done?" }
            textarea {
                rows: "5",
                value: "{text}",
                // Autofocus because this dialog exists only to be typed into: it opened as the result of a
                // gesture, not of somebody looking for a form.
                autofocus: true,
                oninput: move |event| text.set(event.value()),
            }
            div { class: "field-hint",
                "A line or two, as Markdown: what changed, and anything the next person should know. Not that it is finished — the column says that. This goes on the task's thread, signed with your name."
            }
        }
    };

    let ok_button = rsx! {
        button {
            class: "btn btn-primary",
            disabled: !can_save,
            onclick: move |_| on_submit.call(text.read().trim().to_string()),
            if feedback.saving { "Landing…" } else { "Land it" }
        }
    };

    super::dialog_template(&title, content, ok_button)
}
