use dioxus::prelude::*;

/// Something the board tried and could not do, in the server's own words.
///
/// A dialog rather than a toast, and for one reason: the messages this shows are sentences worth reading —
/// "RMS-42 is part of RMS-G7, which is closed — re-open the goal first" — and anything that fades is
/// something half the readers never saw. Read-only, so it closes by the cross, like every other dialog
/// with nothing to save.
#[component]
pub fn MessageDialog(title: String, text: String) -> Element {
    let content = rsx! {
        div { class: "task-view-text", "{text}" }
    };

    super::dialog_template_read_only(&title, content, None)
}
