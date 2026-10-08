use dioxus::prelude::*;
use dioxus_utils::RenderState;
use task_manager_shared::projects::ProjectResponse;
use task_manager_shared::releases::ReleaseResponse;

use crate::states::AppState;

use super::{
    ComponentState, DONE, IN_PROGRESS, NOT_ON_ENV, ON_ENV, envs_to_offer, filter_asks,
    includes_service, matches_done_filter, matches_env_filter, services_to_offer,
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

    // Created from what this browser kept of the screen — the board it was on and the three filters —
    // so the first thing drawn is the screen as it was left. That is the only read of storage here:
    // nothing below, in the render body or in an effect, asks it anything, and it is written by the
    // state methods that change what is stored. See `ComponentState::new`.
    let mut cs = use_signal(ComponentState::new);

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

    // Watched, or this screen would have no channel for the pushes it is drawn from. Only that: which
    // board is remembered is written by the state when a board is chosen, not from here — an effect
    // would pay for a storage write on every change of anything in the state.
    use_effect(move || {
        let prefix = cs.read().selected.clone();

        if !prefix.is_empty() {
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

    // The filters offer what the loaded releases name, so the header is built after the read is asked for
    // and with nothing to offer while it is still out — nothing but the environment already being
    // filtered by, which stays in its control whatever the board holds.
    let (services, envs) = match &releases {
        Ok(releases) => (
            services_to_offer(releases, cs_ra.service_filter.as_str()),
            envs_to_offer(releases, cs_ra.env_filter.as_str()),
        ),
        Err(_) => (
            services_to_offer(&[], cs_ra.service_filter.as_str()),
            envs_to_offer(&[], cs_ra.env_filter.as_str()),
        ),
    };

    let header = rsx! {
        RenderHeader { projects: projects.to_vec(), services, envs, cs }
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
    let env_filter = cs_ra.env_filter.as_str();
    let done_filter = cs_ra.done_filter.as_str();

    let shown: Vec<ReleaseResponse> = releases
        .iter()
        .filter(|release| includes_service(release, service_filter))
        .filter(|release| matches_env_filter(release, env_filter))
        .filter(|release| matches_done_filter(release, done_filter))
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
                    // The row a goal's dialog draws its releases with too — see `ReleaseRow`. Here it
                    // names the goal each one shipped, which is what says which feature it was.
                    crate::dialogs::ReleaseRow {
                        key: "{release.id}",
                        release: release.clone(),
                        open: expanded.contains(&release.id),
                        with_goals: true,
                        on_toggle: move |id: String| cs.write().toggle(&id),
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
                        // The board is chosen by the state, from what it read of storage when it was
                        // created — the one this browser was last on, when the reader can still see it.
                        cs.write().projects_loaded(response.projects);

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
    envs: Vec<String>,
    cs: Signal<ComponentState>,
) -> Element {
    let cs_ra = cs.read();
    let selected_prefix = cs_ra.selected.clone();
    let service_filter = cs_ra.service_filter.clone();
    let env_filter = cs_ra.env_filter.clone();
    let done_filter = cs_ra.done_filter.clone();
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
                // Where a release has got to. One is recorded when it ships anywhere, so the unfiltered
                // list is everything that went out; `On Prod` narrows it to what is live — and, with a
                // service picked beside it, to the version of it users are on. Each environment is
                // offered both ways round, because "what has not reached Prod yet" is the list somebody
                // rolling out works from, and it is not the same as "what is on Dev".
                select {
                    onchange: move |event| cs.write().set_env_filter(event.value()),
                    option { value: "", selected: env_filter.is_empty(), "Any environment" }
                    for env in envs.iter() {
                        option {
                            key: "on-{env}",
                            value: "{ON_ENV}{env}",
                            selected: filter_asks(&env_filter, ON_ENV, env),
                            "On {env}"
                        }
                    }
                    for env in envs.iter() {
                        option {
                            key: "not-{env}",
                            value: "{NOT_ON_ENV}{env}",
                            selected: filter_asks(&env_filter, NOT_ON_ENV, env),
                            "Not on {env} yet"
                        }
                    }
                }
                // Whether a release's rollout is over. `In progress` is the short list this screen is
                // opened for by whoever is rolling things out: everything recorded that still has
                // somewhere to go. It is not the same question as an environment — which environments
                // there are differs from board to board, and "everywhere it is going" is somebody's
                // statement about a release, not a label on it.
                select {
                    onchange: move |event| cs.write().set_done_filter(event.value()),
                    option { value: "", selected: done_filter.is_empty(), "Any state" }
                    option {
                        value: IN_PROGRESS,
                        selected: done_filter == IN_PROGRESS,
                        "In progress"
                    }
                    option { value: DONE, selected: done_filter == DONE, "Done" }
                }
            }
        }
    }
}
