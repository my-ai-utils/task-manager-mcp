use dioxus::prelude::*;

/// The shell every signed-in screen sits in: the bar of product areas across the top, the work area
/// under it at the full width of the window.
#[component]
pub fn ContentPanel(active: &'static str, children: Element) -> Element {
    rsx! {
        super::TopBar { active }
        div { class: "main-content", {children} }
    }
}
