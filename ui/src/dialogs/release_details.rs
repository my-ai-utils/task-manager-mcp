use dioxus::prelude::*;
use task_manager_shared::releases::{
    ReleaseResponse, has_settings_update, is_production_env, moment_for_display, release_page_path,
};

/// The inside of one release: what it is, what was deployed, and what changed.
///
/// One component for the three places a release is read in full — unfolded on the Releases screen, under
/// the goal it shipped, and on its own page — because it is one thing, and a second rendering of it would
/// be the first place a new field showed up in one and not the other.
///
/// The services come BEFORE the notes. They are the part that is looked up — which version, which commit —
/// where the notes are read once; and a settings change, which sits with its service, is the one line here
/// that somebody has to act on.
///
/// Read-only, like everything else: a release is recorded and corrected through `/mcp`.
#[component]
pub fn ReleaseDetails(release: ReleaseResponse) -> Element {
    // Rendered rather than shown as source, and escaped rather than trusted — the same call a task's text
    // goes through, for the same reason.
    let description_html = super::md_to_html(&release.description);
    let notes_html = super::md_to_html(&release.release_notes);

    let has_description = !release.description.trim().is_empty();
    let has_notes = !release.release_notes.trim().is_empty();

    // Where the build can be looked at is a column of its own, and only when some service has one: most
    // boards build by hand, and a column of blanks on every release of theirs would be a heading that
    // says something is missing.
    let has_links = release
        .services
        .iter()
        .any(|itm| !itm.release_link.trim().is_empty());

    let columns = if has_links { 5 } else { 4 };

    rsx! {
        div { class: "release-details",
            if has_description {
                div { class: "release-text md", dangerous_inner_html: "{description_html}" }
            }

            if release.services.is_empty() {
                // Said rather than left blank: a release with no services is a title and a date, which is
                // either a release still being written down or one that was never finished.
                div { class: "field-hint", "No services are recorded in this release yet." }
            } else {
                table { class: "release-services",
                    thead {
                        tr {
                            th { "Microservice" }
                            th { "Version" }
                            th { "Git hash" }
                            if has_links {
                                th { "Build" }
                            }
                            th { "Went out" }
                        }
                    }
                    // A `tbody` per service, which HTML allows: a service is its row plus the notes under
                    // it, and one body each keeps those together under a single key.
                    for service in release.services.iter() {
                        tbody { key: "{service.microservice_id}",
                            tr {
                                td { class: "release-service-id", "{service.microservice_id}" }
                                td { class: "release-service-version", "{service.version}" }
                                // Whole, not abbreviated: this is the value somebody copies into
                                // `git show`, and seven characters with the rest in a tooltip cannot be
                                // copied.
                                td { class: "release-service-hash", "{service.git_hash}" }
                                if has_links {
                                    td { class: "release-service-link",
                                        ReleaseBuildLink { link: service.release_link.clone() }
                                    }
                                }
                                td { class: "release-service-moment",
                                    "{moment_for_display(service.datetime_unix_seconds)}"
                                }
                            }

                            // Its own row and its own colour, never a line in the notes: whether the
                            // settings have to change is the one fact here that is acted on rather than
                            // read, and the reason it is a separate field at all.
                            if !service.settings_update_note.trim().is_empty() {
                                tr { class: "release-service-settings",
                                    td { colspan: "{columns}",
                                        div { class: "release-note-label", "Settings update" }
                                        div {
                                            class: "md",
                                            dangerous_inner_html: "{super::md_to_html(&service.settings_update_note)}",
                                        }
                                    }
                                }
                            }

                            if !service.description.trim().is_empty() {
                                tr { class: "release-service-description",
                                    td { colspan: "{columns}",
                                        div {
                                            class: "md",
                                            dangerous_inner_html: "{super::md_to_html(&service.description)}",
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if has_notes {
                div { class: "release-note-label", "Release notes" }
                div { class: "release-text md", dangerous_inner_html: "{notes_html}" }
            }

            // Last, as on a task and a goal: the notes above say what changed, and this is what was said
            // about the rollout itself. Nothing at all when nobody has said anything — most releases go
            // out without comment, and an empty heading on each would read as something missing.
            if !release.comments.is_empty() {
                div { class: "release-thread",
                    div { class: "task-view-thread-header",
                        "Comments"
                        span { class: "board-column-count", "{release.comments.len()}" }
                    }
                    for (index , comment) in release.comments.iter().enumerate() {
                        div { class: "task-view-comment", key: "{index}",
                            div { class: "task-view-comment-who", "{comment.who}" }
                            div { class: "md", dangerous_inner_html: "{super::md_to_html(&comment.text)}" }
                        }
                    }
                }
            }
        }
    }
}

/// Where the build of a service's version can be looked at — the GitHub release, or the run that built
/// the image.
///
/// An anchor ONLY for something that is a web address. The server refuses anything else on the way in,
/// and this checks again, because whatever lands in `href` is followed on a click: a value that got past
/// that check — an older row, a file somebody edited — is drawn as text, which can be read and cannot be
/// run.
///
/// A new tab, since the reader is in the middle of a release and the build is a detour; `noopener` so the
/// page it opens is handed nothing of this one. What is shown is the address without its scheme — it says
/// WHERE the build is, which a bare "open" would not, and the cell clips it rather than widening the table.
#[component]
fn ReleaseBuildLink(link: String) -> Element {
    let link = link.trim().to_string();

    if link.is_empty() {
        return rsx! {};
    }

    let shown = link
        .strip_prefix("https://")
        .or_else(|| link.strip_prefix("http://"));

    match shown {
        Some(shown) => rsx! {
            a {
                href: "{link}",
                target: "_blank",
                rel: "noopener noreferrer",
                title: "{link}",
                "{shown} ↗"
            }
        },
        None => rsx! {
            span { title: "Not a web address, so it is not drawn as a link", "{link}" }
        },
    }
}

/// Where a release is out: one chip per environment, in the order it reached them.
///
/// Beside the settings flag and deliberately unlike it: that one is amber because it asks somebody to do
/// something, these are quiet because they report. Production alone is filled, in the green the board
/// already uses for `Done` — "is it live" is the question a list of releases is most often scanned for,
/// and it is the same statement about a release that `Done` is about a goal.
///
/// Nothing at all for a release nobody has said anything about: that is how every release starts, so its
/// absence is the ordinary state and not a warning.
#[component]
pub fn ReleaseEnvs(release: ReleaseResponse) -> Element {
    rsx! {
        for env in release.envs.iter() {
            span {
                key: "{env}",
                class: if is_production_env(env) { "release-env prod" } else { "release-env" },
                title: "Out on {env}",
                "{env}"
            }
        }
    }
}

/// The way from a release to its own page, in a new tab.
///
/// A release has an address — `release/{project}/{id}` — so that it can be handed to somebody as a link,
/// and this is how a reader gets one: the page it opens has the address in its bar. A plain anchor rather
/// than a route push, which is what makes "new tab" the browser's doing: the middle button and the
/// context menu work on it as on any link.
///
/// `stop_propagation` because on the Releases screen it sits inside the head, and the head folds the
/// release — one click must do one thing.
#[component]
pub fn ReleaseOpenLink(release: ReleaseResponse) -> Element {
    rsx! {
        a {
            class: "release-open-link",
            href: "{release_page_path(&release)}",
            target: "_blank",
            rel: "noopener",
            title: "Open {release.id} on a page of its own, in a new tab — the address there is the link to share",
            onclick: move |event| event.stop_propagation(),
            "↗"
        }
    }
}

/// The mark on a release some service of which needs its settings changed.
///
/// Drawn wherever a release is named in one line, so the fact is visible BEFORE the release is opened —
/// a note that only shows once you have unfolded the right row is a note that gets found after the deploy.
/// Nothing at all when no service carries one: a badge on every release would be a badge nobody reads.
#[component]
pub fn ReleaseSettingsFlag(release: ReleaseResponse) -> Element {
    if !has_settings_update(&release) {
        return rsx! {};
    }

    rsx! {
        span {
            class: "release-settings-flag",
            title: "A service in this release needs its settings changed — open it to see which",
            "⚙ Settings"
        }
    }
}
