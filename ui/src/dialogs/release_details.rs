use dioxus::prelude::*;
use task_manager_shared::releases::{ReleaseResponse, has_settings_update, moment_for_display};

/// The inside of one release: what it is, what was deployed, and what changed.
///
/// One component for the two places a release is read in full — unfolded on the Releases screen, and under
/// the goal it shipped — because it is one thing, and two renderings of it would be the first place a new
/// field showed up in one and not the other.
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

    rsx! {
        div { class: "release-details",
            // Said in words where the folded row says it with a badge, and with the moment: "on prod"
            // is the question, "since when" is the next one. Nothing when it is not there yet — every
            // release starts that way, and a line saying so on each would be a line nobody reads.
            if let Some(since) = release.released_on_prod_unix_seconds {
                div { class: "release-on-prod",
                    "On production since {moment_for_display(since)}"
                }
            }

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
                                td { class: "release-service-moment",
                                    "{moment_for_display(service.datetime_unix_seconds)}"
                                }
                            }

                            // Its own row and its own colour, never a line in the notes: whether the
                            // settings have to change is the one fact here that is acted on rather than
                            // read, and the reason it is a separate field at all.
                            if !service.settings_update_note.trim().is_empty() {
                                tr { class: "release-service-settings",
                                    td { colspan: "4",
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
                                    td { colspan: "4",
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

/// The mark on a release that is out on production.
///
/// Beside the settings flag and deliberately unlike it: that one is amber because it asks somebody to do
/// something, this one is the green the board already uses for `Done`, because it says something has
/// been done. Nothing when the release has not reached production — which is every release at first, so
/// its absence is the ordinary state and not a warning.
#[component]
pub fn ReleaseProdFlag(release: ReleaseResponse) -> Element {
    let Some(since) = release.released_on_prod_unix_seconds else {
        return rsx! {};
    };

    rsx! {
        span {
            class: "release-prod-flag",
            title: "Out on production since {moment_for_display(since)}",
            "Prod"
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
