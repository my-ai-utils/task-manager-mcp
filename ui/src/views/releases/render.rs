use dioxus::prelude::*;
use dioxus_utils::RenderState;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::releases::{ReleaseResponse, moment_for_display};

use crate::states::AppState;

use super::{
    ComponentState, NOT_ON_PROD, ON_PROD, SERVICES_ON_A_ROW, includes_service,
    matches_prod_filter, microservices_of,
};

/// What has gone out, newest first.
///
/// Home answers "what is on the board" and Goals "how is each goal going"; this answers the question that
/// comes after both — what is OUT, and in which version of which microservice. One row per release, folded
/// to a line and opened to read.
///
/// Read-only, like its neighbours: a release is recorded through `/mcp`, and this screen owes the reader an
/// accurate history rather than controls.
#[component]
pub fn RenderReleases() -> Element {
    let app_state = consume_context::<Signal<AppState>>();

    let mut cs = use_signal(ComponentState::default);

    // A push carries every release of the project, so once the screen is up it is served from the socket
    // alone: a release an agent has just recorded appears without anybody re-reading anything, and an
    // unfolded row stays unfolded across the repaint.
    use_effect(move || {
        let app_ra = app_state.read();
        let _revision = app_ra.board_revision;
        let push = app_ra.board_push.clone();
        drop(app_ra);

        match push {
            Some(snapshot) if snapshot.project == cs.peek().selected => {
                cs.write().board_pushed(snapshot.releases);
            }
            Some(_) => {}
            None if cs.peek().releases.has_value() => cs.write().board_invalidated(),
            None => {}
        }
    });

    // Remembered where Home and Goals remember it, so the three tabs stay on one board — and watched, or
    // this screen would have no channel for the pushes it is drawn from.
    use_effect(move || {
        let prefix = cs.read().selected.clone();

        if !prefix.is_empty() {
            crate::web::storage::save_last_project(&prefix);
            crate::web::watch_project(&prefix);
        }
    });

    let cs_ra = cs.read();

    let projects = match get_projects(cs, &cs_ra) {
        Ok(projects) => projects,
        Err(element) => return element,
    };

    if projects.is_empty() {
        return rsx! {
            div { class: "page-header",
                h1 { class: "page-title", "Releases" }
            }
            div { class: "empty-note",
                "You are not on any project yet. Ask an admin to add you to one."
            }
        };
    }

    let releases = get_releases(cs, &cs_ra);

    // The filter offers what the loaded releases name, so the header is built after the read is asked for
    // and with nothing to offer while it is still out.
    let services = match &releases {
        Ok(releases) => microservices_of(releases),
        Err(_) => Vec::new(),
    };

    let header = rsx! {
        RenderHeader { projects: projects.to_vec(), services, cs }
    };

    let releases = match releases {
        Ok(releases) => releases,
        Err(element) => {
            return rsx! {
                div { class: "releases-page",
                    {header}
                    {element}
                }
            };
        }
    };

    let service_filter = cs_ra.service_filter.as_str();
    let prod_filter = cs_ra.prod_filter.as_str();

    let shown: Vec<ReleaseResponse> = releases
        .iter()
        .filter(|release| includes_service(release, service_filter))
        .filter(|release| matches_prod_filter(release, prod_filter))
        .cloned()
        .collect();

    // Two different empties, and a screen that said "nothing has been released" where a filter is what
    // emptied it would be lying to the person holding the filter.
    let empty_note: Option<String> = if !shown.is_empty() {
        None
    } else if releases.is_empty() {
        Some(
            "Nothing has been released on this board yet. A release is recorded through MCP — ask an agent to record one when work ships."
                .to_string(),
        )
    } else {
        Some("No release on this board matches these filters.".to_string())
    };

    let expanded = cs_ra.expanded.clone();
    drop(cs_ra);

    rsx! {
        div { class: "releases-page",
            {header}

            if let Some(note) = empty_note {
                div { class: "empty-note", "{note}" }
            }

            div { class: "releases-list",
                for release in shown.iter() {
                    RenderRelease {
                        key: "{release.id}",
                        release: release.clone(),
                        open: expanded.contains(&release.id),
                        cs,
                    }
                }
            }
        }
    }
}

fn get_projects(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[ProjectResponse], Element> {
    match cs_ra.projects.as_ref() {
        RenderState::None => {
            spawn(async move {
                cs.write().projects.set_loading();

                match crate::api::get_projects().await {
                    Ok(response) => {
                        // The same choice Home and Goals make, for the same reasons: the board this
                        // browser was last on when it is still one the reader can see, else the first
                        // live one, else whatever there is.
                        let remembered = crate::web::storage::get_last_project();

                        let initial = remembered
                            .filter(|prefix| {
                                response
                                    .projects
                                    .iter()
                                    .any(|itm| itm.prefix.eq_ignore_ascii_case(prefix))
                            })
                            .or_else(|| {
                                response
                                    .projects
                                    .iter()
                                    .find(|itm| !itm.archived)
                                    .or_else(|| response.projects.first())
                                    .map(|itm| itm.prefix.clone())
                            })
                            .unwrap_or_default();

                        let mut cs_wa = cs.write();
                        cs_wa.selected = initial;
                        cs_wa.projects.set_loaded(response.projects);
                        drop(cs_wa);

                        // Started by THIS screen too: somebody who opens /releases directly would
                        // otherwise have no channel for changes at all. Idempotent.
                        crate::start_ws();
                    }
                    Err(err) => cs.write().projects.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(projects) => Ok(projects.as_slice()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

fn get_releases(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[ReleaseResponse], Element> {
    if cs_ra.selected.is_empty() {
        return Ok(&[]);
    }

    match cs_ra.releases.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected.clone();

            spawn(async move {
                cs.write().releases.set_loading();

                match crate::api::get_releases(&project).await {
                    Ok(response) => cs.write().releases.set_loaded(response.releases),
                    Err(err) => cs.write().releases.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(releases) => Ok(releases.as_slice()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

fn render_loading() -> Element {
    rsx! {
        div { class: "loading-note", "Loading…" }
    }
}

fn render_error(message: &str) -> Element {
    rsx! {
        div { class: "error-note", "{message}" }
    }
}

#[component]
fn RenderHeader(
    projects: Vec<ProjectResponse>,
    services: Vec<String>,
    cs: Signal<ComponentState>,
) -> Element {
    let cs_ra = cs.read();
    let selected_prefix = cs_ra.selected.clone();
    let service_filter = cs_ra.service_filter.clone();
    let prod_filter = cs_ra.prod_filter.clone();
    drop(cs_ra);

    let mut cs = cs;

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Releases" }
            div { class: "project-picker",
                // `selected` on the option rather than `value` on the select, or a remembered project is
                // restored into the state and not shown in the control.
                select {
                    onchange: move |event| cs.write().select(event.value()),
                    // Archived boards are left out, except the one currently open — the control follows
                    // `selected`, and an open board with no option has none to follow.
                    for project in projects.iter().filter(|itm| !itm.archived || itm.prefix == selected_prefix) {
                        option {
                            value: "{project.prefix}",
                            selected: project.prefix == selected_prefix,
                            "{project.prefix} · {project.name}"
                        }
                    }
                }
                // Which service's history is on screen. With the list newest first, picking one puts
                // the version of it that is out at the top — the question this screen is most often
                // opened to answer. "Any" is spelled out, as on Goals: a filter whose off state has no
                // name is one people are not sure they have turned off.
                select {
                    onchange: move |event| cs.write().set_service_filter(event.value()),
                    option { value: "", selected: service_filter.is_empty(), "Any service" }
                    for service in services.iter() {
                        option {
                            value: "{service}",
                            selected: *service == service_filter,
                            "{service}"
                        }
                    }
                }
                // What has actually reached production. A release is recorded when it ships anywhere,
                // so the unfiltered list is everything that went out; this is what narrows it to what
                // is live — and, with a service picked beside it, to the version of it users are on.
                select {
                    onchange: move |event| cs.write().set_prod_filter(event.value()),
                    option { value: "", selected: prod_filter.is_empty(), "Any stage" }
                    option { value: ON_PROD, selected: prod_filter == ON_PROD, "On prod" }
                    option {
                        value: NOT_ON_PROD,
                        selected: prod_filter == NOT_ON_PROD,
                        "Not on prod yet"
                    }
                }
            }
        }
    }
}

/// One release, folded to a line or open.
#[component]
fn RenderRelease(release: ReleaseResponse, open: bool, cs: Signal<ComponentState>) -> Element {
    let mut cs = cs;

    let release_id = release.id.clone();
    let date = moment_for_display(release.date_unix_seconds);

    let more_services = release.services.len().saturating_sub(SERVICES_ON_A_ROW);

    // Every service by name and version, for the tooltip on the count — the folded row shows three and
    // this is what says which the others are without opening it.
    let all_services = release
        .services
        .iter()
        .map(|itm| format!("{} {}", itm.microservice_id, itm.version))
        .collect::<Vec<_>>()
        .join("\n");

    rsx! {
        div { class: "release-card",
            div {
                class: "release-head",
                onclick: move |_| cs.write().toggle(&release_id),

                span { class: "goal-caret", if open { "▾" } else { "▸" } }

                div { class: "release-head-text",
                    // The handle first and the title after it, in the order a goal's row and a task's
                    // sticker put them — this is the id that goes into `add_releases` on a goal.
                    span { class: "goal-id", "{release.id}" }
                    div { class: "release-title", "{release.title}" }
                }

                div { class: "release-meta",
                    // The goal it shipped, in that goal's own colour — by id alone. The name is in the
                    // tooltip and in the open release: on one line it was competing for room with the
                    // versions, and a release's own title usually says what the goal's would.
                    for goal in release.goals.iter() {
                        span {
                            class: "release-goal",
                            key: "{goal.id}",
                            style: "border-left-color: {KindColor::parse_or_default(&goal.color).hex()}",
                            title: "{goal.id} · {goal.name}",
                            span { class: "goal-id", "{goal.id}" }
                        }
                    }

                    crate::dialogs::ReleaseProdFlag { release: release.clone() }
                    crate::dialogs::ReleaseSettingsFlag { release: release.clone() }

                    for service in release.services.iter().take(SERVICES_ON_A_ROW) {
                        span {
                            class: "release-service-chip",
                            key: "{service.microservice_id}",
                            title: "{service.microservice_id} {service.version}",
                            span { class: "release-service-chip-name", "{service.microservice_id}" }
                            span { class: "release-service-chip-version", "{service.version}" }
                        }
                    }

                    if more_services > 0 {
                        span { class: "release-more", title: "{all_services}", "+{more_services}" }
                    }

                    // Drawn only when there is one, like the same count on a goal's row.
                    if !release.comments.is_empty() {
                        span {
                            class: "goal-comments",
                            title: "{release.comments.len()} notes on the thread — open the release to read them",
                            "💬 {release.comments.len()}"
                        }
                    }

                    span { class: "release-date", "{date}" }
                }
            }

            if open {
                div { class: "release-body",
                    // Which feature this was, in full. Here and not inside `ReleaseDetails`, because
                    // that body is also drawn under the goal itself, where naming the goal would be
                    // telling the reader what dialog they are in.
                    if !release.goals.is_empty() {
                        div { class: "release-goals",
                            for goal in release.goals.iter() {
                                span {
                                    class: "release-goal",
                                    key: "{goal.id}",
                                    style: "border-left-color: {KindColor::parse_or_default(&goal.color).hex()}",
                                    span { class: "goal-id", "{goal.id}" }
                                    span { "{goal.name}" }
                                }
                            }
                        }
                    }

                    crate::dialogs::ReleaseDetails { release: release.clone() }
                }
            }
        }
    }
}
