use dioxus::prelude::*;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::releases::{
    ReleaseGoalResponse, ReleaseResponse, is_done, moment_for_display,
};

use crate::states::AppState;

use super::DialogState;

/// How many services a folded release names before the rest are counted rather than listed.
///
/// A row is one line. Three chips say which release this is for nearly every release there is — one
/// feature rarely touches more — and the count after them says there is more without widening the row.
pub const SERVICES_ON_A_ROW: usize = 3;

/// One release as a row of a list: folded to a line, or open.
///
/// **One component for both lists a release is a row of** — the Releases screen, and the releases a goal
/// went out in, inside that goal's dialog. They are the same row answering the same question, "which one
/// is it and where has it got to", and a second drawing of it would be the first place a new chip showed
/// up in one list and not in the other.
///
/// The row holds no state of its own: which rows are open belongs to whoever draws the list, so that a
/// repaint — a push rebuilds the whole list — does not fold up what somebody was reading. `on_toggle` is
/// handed the release's id and nothing else.
///
/// `with_goals` is the one difference between the two lists. On the Releases screen the goal a release
/// shipped is what says which feature it was; under a goal it would be the dialog telling the reader what
/// dialog they are in.
#[component]
pub fn ReleaseRow(
    release: ReleaseResponse,
    open: bool,
    with_goals: bool,
    on_toggle: EventHandler<String>,
) -> Element {
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
        // A closed release recedes, the way a closed goal does: it is history now, and the rows still in
        // flight are the ones somebody has to do something about.
        div { class: if is_done(&release) { "release-card done" } else { "release-card" },
            div {
                class: "release-head",
                onclick: move |_| on_toggle.call(release_id.clone()),

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
                    if with_goals {
                        for goal in release.goals.iter() {
                            ReleaseGoalChip {
                                key: "{goal.id}",
                                goal: goal.clone(),
                                project: release.project.clone(),
                                named: false,
                            }
                        }
                    }

                    super::ReleaseEnvs { release: release.clone() }
                    super::ReleaseDoneFlag { release: release.clone() }
                    super::ReleaseSettingsFlag { release: release.clone() }

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

                    // Last, so it is in one place down the list: the way to this release's own page,
                    // and so to an address that can be handed to somebody.
                    super::ReleaseOpenLink { release: release.clone() }
                }
            }

            if open {
                div { class: "release-body",
                    // Which feature this was, in full. Here and not inside `ReleaseDetails`, because
                    // that body is also drawn under the goal itself, where naming the goal would be
                    // telling the reader what dialog they are in.
                    if with_goals && !release.goals.is_empty() {
                        div { class: "release-goals",
                            for goal in release.goals.iter() {
                                ReleaseGoalChip {
                                    key: "{goal.id}",
                                    goal: goal.clone(),
                                    project: release.project.clone(),
                                    named: true,
                                }
                            }
                        }
                    }

                    super::ReleaseDetails { release: release.clone() }
                }
            }
        }
    }
}

/// A goal a release shipped, as a chip edged in that goal's colour — and the way into the goal.
///
/// `named` is the one difference between the places it is drawn: the folded row has room for the id only
/// and says the name in a tooltip, the open release and the release's own page spell it out.
#[component]
pub fn ReleaseGoalChip(goal: ReleaseGoalResponse, project: String, named: bool) -> Element {
    let app_state = consume_context::<Signal<AppState>>();

    let color = KindColor::parse_or_default(&goal.color).hex();
    let tooltip = if named {
        String::new()
    } else {
        format!("{} · {}", goal.id, goal.name)
    };

    let goal_id = goal.id.clone();

    rsx! {
        span {
            class: "release-goal",
            style: "border-left-color: {color}",
            title: "{tooltip}",
            span { class: "goal-id", "{goal.id}" }
            if named {
                span { "{goal.name}" }
            }

            // The eye a goal's own row ends with, opening the dialog that one opens. `stop_propagation`
            // for the reason it is there too: on the folded row this chip sits inside the head, the head
            // folds the release, and one click must do one thing.
            button {
                class: "sticker-view",
                title: "Read this goal and its comments",
                onclick: move |event| {
                    event.stop_propagation();
                    open_goal(app_state, project.clone(), goal_id.clone());
                },
                "👁"
            }
        }
    }
}

/// Opens a goal a release names, in the dialog the eye on that goal's own row opens.
///
/// A release holds its goal by id, name and colour — enough for the chip, not for the dialog. The rest is
/// the board, and it is usually already here: the push the screen is drawn from carries the goals and the
/// tasks beside the releases, so the dialog opens with no request and from the same revision as the row
/// that was clicked.
///
/// **Usually, and not always — a release is exactly the thing that outlives its goal.** A push carries the
/// LIVE goals; one closed longer ago than the project's archive window is not among them, and that is the
/// ordinary state of a goal whose release is a month old. So a goal the push does not hold is asked of the
/// server, archive included — as is every goal when there is no push to look in, which is the first
/// moment of a cold load and any tab whose socket has dropped.
fn open_goal(app_state: Signal<AppState>, project: String, goal_id: String) {
    let pushed = {
        let app_ra = app_state.read();

        app_ra
            .board_push
            .as_ref()
            .filter(|snapshot| snapshot.project == project)
            .and_then(|snapshot| {
                crate::views::goals::find_goal_with_status(
                    &snapshot.goals,
                    &snapshot.tasks,
                    &goal_id,
                )
            })
    };

    if let Some((goal, status)) = pushed {
        super::open(view_goal(goal, status));
        return;
    }

    spawn(async move {
        let dialog = match read_goal(&project, &goal_id).await {
            Ok(Some((goal, status))) => view_goal(goal, status),
            // A release does not list a goal that has been deleted, so this is a goal deleted between
            // the list being read and the eye being clicked. Said, rather than a click that does nothing.
            Ok(None) => DialogState::Message {
                title: goal_id,
                text: "This goal is not on the board any more.".to_string(),
            },
            Err(message) => DialogState::Message {
                title: goal_id,
                text: message,
            },
        };

        super::open(dialog);
    });
}

fn view_goal(goal: GoalResponse, status: &str) -> DialogState {
    DialogState::ViewGoal {
        goal,
        status: status.to_string(),
    }
}

/// One goal by id, read from the server with the tasks its status is derived from.
///
/// Both lists whole, archive included: the goal is being asked for BECAUSE it may have left the live
/// list, and its status is a statement about its tasks — the same ones the Goals screen reads, so the two
/// cannot answer differently. Asked for together, since neither needs the other to be sent.
async fn read_goal(
    project: &str,
    goal_id: &str,
) -> Result<Option<(GoalResponse, &'static str)>, String> {
    let (goals, tasks) = futures::join!(
        crate::api::get_goals(project, true),
        crate::api::get_tasks(project, true),
    );

    let goals = goals.map_err(|err| err.message)?.goals;
    let tasks = tasks.map_err(|err| err.message)?.tasks;

    Ok(crate::views::goals::find_goal_with_status(
        &goals, &tasks, goal_id,
    ))
}
