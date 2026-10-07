use dioxus::prelude::*;

/// Hand the server a key for one connected repository.
///
/// **Reached from the tree, where the problem is visible.** A connection that says "needs a key" is
/// saying the one thing somebody can fix in ten seconds, and making them go to Projects setup to do it
/// puts the fix on a different screen from the symptom — on a screen most readers have no reason to open.
///
/// The box is always empty when this opens, and not because nothing was typed before: the server will
/// not say what key it is holding, to anybody, ever. So there is nothing to prefill and nothing this
/// dialog could show you about the current one.
#[component]
pub fn GithubKeyDialog(connection: String, on_submit: EventHandler<String>) -> Element {
    let mut cs = use_signal(String::new);

    let feedback = super::feedback();
    let key = cs.read().clone();

    let content = rsx! {
        div { class: "form-row",
            label { "Key for {connection}" }
            input {
                r#type: "password",
                placeholder: "a GitHub token for this repository",
                value: "{key}",
                oninput: move |event| cs.set(event.value()),
            }
            div { class: "field-hint",
                "Held in this server's memory and written to no table — so a database dump carries no credential, and the key has to be given again after every restart. What a restart loses is the key, not the files: the clone is still on this server's disk, so listing and reading never stopped and only fetching and pushing wait. The same key does both: Contents: Read on a fine-grained token keeps the copy current and has its pushes rejected, Contents: Read and write pushes, and a classic token's repo scope is both. It serves everyone on this project for as long as it is held, so whatever it can do to the repository, they can."
            }

            if !feedback.error.is_empty() {
                div { class: "error-note", "{feedback.error}" }
            }
        }
    };

    let ok = rsx! {
        button {
            class: "btn btn-primary",
            disabled: key.trim().is_empty() || feedback.saving,
            onclick: move |_| on_submit.call(cs.read().trim().to_string()),
            if feedback.saving { "Sending…" } else { "Hand it over" }
        }
    };

    super::dialog_template("Key for a connected repository", content, ok)
}
