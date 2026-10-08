use dioxus::prelude::*;
use dioxus_utils::RenderState;
use task_manager_shared::releases::{ReleaseResponse, is_done, moment_for_display};

use crate::AppRoute;
use crate::states::AppState;

use super::ComponentState;

/// One release, on a page of its own: `release/{project}/{release}`.
///
/// **This is what makes a release something that can be linked to.** The Releases screen is a list and
/// remembers its board rather than naming it in the address, so there was nothing to paste into a chat
/// that would land somebody on "this one". Here the address IS the release — the board's prefix, then the
/// release's id or its bare number — and it is opened cold: nothing is assumed to be loaded, and the one
/// read it makes is by those two halves.
///
/// Drawn with the pieces the list and a goal's dialog draw a release with, so the three cannot disagree
/// about what a release looks like. Read-only, like them: a release is recorded and corrected through
/// `/mcp`.
#[component]
pub fn RenderReleasePage(project: String, release: String) -> Element {
    let app_state = consume_context::<Signal<AppState>>();

    let mut cs = use_signal({
        let project = project.clone();
        let release = release.clone();
        move || ComponentState::new(project, release)
    });

    // `use_reactive!` because a prop is not a signal — a plain `use_effect` closes over the first render's
    // address and never hears of the next one. A link to another release, followed from this page, changes
    // the address without building a new page.
    use_effect(use_reactive!(|(project, release)| {
        if !cs.peek().is_at(&project, &release) {
            cs.write().open(&project, &release);
        }
    }));

    // Live, like every screen: a push carries every release of the board, so a label added when this one
    // reaches production arrives without a reload.
    //
    // Woken by ANY change of the app's state, and acting only on a push — a revision that moved. The
    // `peek` is what keeps the rest from being a write: this page starts the socket itself, which changes
    // the app's state with nothing pushed, and see `board_changed` for what that used to cost.
    use_effect(move || {
        let app_ra = app_state.read();
        let revision = app_ra.board_revision;
        let push = app_ra.board_push.clone();
        drop(app_ra);

        if cs.peek().seen_revision != revision {
            cs.write().board_changed(revision, push.as_ref());
        }
    });

    // Watched, or the page would have no channel for the pushes it is kept current by. Remembering the
    // board is not done here: the state does that when the release arrives — see `release_loaded`.
    use_effect(move || {
        let prefix = cs
            .read()
            .release
            .try_unwrap_as_loaded()
            .map(|itm| itm.project.clone());

        if let Some(prefix) = prefix {
            crate::web::watch_project(&prefix);
        }
    });

    // The tab is named after the release while this page is up, and gets its own name back afterwards:
    // the router changes screens without loading a page, so nothing else would put it back.
    let tab_title = use_hook(crate::web::document_title);

    use_effect(move || {
        let named = cs
            .read()
            .release
            .try_unwrap_as_loaded()
            .map(|itm| format!("{} · {}", itm.id, itm.title));

        if let Some(named) = named {
            crate::web::set_document_title(&named);
        }
    });

    use_drop(move || crate::web::set_document_title(&tab_title));

    let cs_ra = cs.read();

    let release = match get_release(cs, &cs_ra) {
        Ok(release) => release,
        Err(element) => return element,
    };

    let date = moment_for_display(release.date_unix_seconds);

    rsx! {
        div { class: "release-page",
            div { class: "page-header",
                h1 { class: "page-title",
                    // The id first and in its own face, as on the list: it is the half of the title
                    // somebody quotes, and the half of the address they can check it against.
                    span { class: "release-page-id", "{release.id}" }
                    "{release.title}"
                }

                // Where this release sits among the others. The list remembers its board rather than
                // taking one from the address, and this page has just set what it remembers.
                Link { class: "release-page-back", to: AppRoute::Releases {},
                    "All releases of {release.project}"
                }
            }

            // Said before anything else, because it changes how everything under it is read: this is
            // what a link somebody kept opens once the release has been deleted.
            if let Some(deleted) = release.deleted_unix_seconds {
                div { class: "error-banner",
                    "This release was deleted on {moment_for_display(deleted)}. It is off the list of releases and off every goal that listed it; it can be brought back through MCP, with releases_update and `deleted: false`."
                }
            }

            div { class: if is_done(release) { "release-card done" } else { "release-card" },
                div { class: "release-head static",
                    div { class: "release-head-text",
                        // Which feature this was — by name here, where the folded row of the list has
                        // room for the id only.
                        if release.goals.is_empty() {
                            span { class: "field-hint", "Not attached to a goal" }
                        }
                        for goal in release.goals.iter() {
                            crate::dialogs::ReleaseGoalChip {
                                key: "{goal.id}",
                                goal: goal.clone(),
                                project: release.project.clone(),
                                named: true,
                            }
                        }
                    }
                    div { class: "release-meta",
                        crate::dialogs::ReleaseEnvs { release: release.clone() }
                        crate::dialogs::ReleaseDoneFlag { release: release.clone() }
                        crate::dialogs::ReleaseSettingsFlag { release: release.clone() }
                        span { class: "release-date", "{date}" }
                    }
                }
                div { class: "release-body",
                    crate::dialogs::ReleaseDetails { release: release.clone() }
                }
            }
        }
    }
}

fn get_release(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&ReleaseResponse, Element> {
    match cs_ra.release.as_ref() {
        RenderState::None => {
            let project = cs_ra.project.clone();
            let release = cs_ra.release_ref.clone();

            spawn(async move {
                cs.write().release.set_loading();

                match crate::api::get_release(&project, &release).await {
                    Ok(response) => {
                        cs.write().release_loaded(response);

                        // Started by THIS page too: somebody who lands here from a link is on no other
                        // screen, and would otherwise have no channel for changes at all. Idempotent.
                        crate::start_ws();
                    }
                    // `format_args!` and not the string itself: `set_error` keeps the `Debug` of what it
                    // is given, and the `Debug` of a string is that string in quotes. This message is
                    // the whole of what the page shows — the server's own sentence about the address —
                    // and it should read as a sentence, not as a value.
                    Err(err) => cs
                        .write()
                        .release
                        .set_error(format_args!("{}", err.message)),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(release) => Ok(release),
        RenderState::Error(err) => Err(render_missing(err)),
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "loading-note", "Loading…" }
    }
}

/// The address named nothing this reader can open.
///
/// The server's own words are shown — it says which half of the address is wrong — with the way out
/// beside them: a link that opens nothing is most often an old one, and the list is where its release
/// would be found if it is there under another number.
fn render_missing(message: &str) -> Element {
    rsx! {
        div { class: "release-page",
            div { class: "page-header",
                h1 { class: "page-title", "Release" }
                Link { class: "release-page-back", to: AppRoute::Releases {}, "All releases" }
            }
            div { class: "empty-note", "{message}" }
        }
    }
}
