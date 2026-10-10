use std::collections::HashMap;

use dioxus::prelude::*;
use dioxus_utils::RenderState;
use task_manager_shared::goals::GoalResponse;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::projects::{COLUMN_ID_DONE, COLUMN_ID_TODO, ProjectResponse};
use task_manager_shared::tasks::TaskResponse;

use crate::states::AppState;

use super::state::{ComponentState, GoalsView};
use super::timeline::{Month, RenderTimeline, merge_with_archive};

/// The same work as Home, seen from the other end.
///
/// Home answers "what is on the board"; this answers "how is each goal going". Read-only for the same
/// reason Home is: goals are opened, renamed and closed through `/mcp`, and this screen owes the reader an
/// accurate picture rather than controls.
#[component]
pub fn RenderGoals() -> Element {
    let app_state = consume_context::<Signal<AppState>>();

    let mut cs = use_signal(ComponentState::new);

    // A push carries the goals and the project's WHOLE task list — archived work included — so this screen
    // is served entirely from it: nothing is requested, nothing is emptied first, and an expanded goal keeps
    // showing its work across the repaint. It used to fetch each expanded goal separately and throw those
    // lists away on every push, which was a round trip and a flicker in a protocol whose whole point is that
    // nothing gets re-read.
    use_effect(move || {
        let app_ra = app_state.read();
        let _revision = app_ra.board_revision;
        let push = app_ra.board_push.clone();
        drop(app_ra);

        match push {
            Some(snapshot) if snapshot.project == cs.peek().selected => {
                let mut write = cs.write();
                write.goals.set_loaded(snapshot.goals);
                write.tasks.set_loaded(snapshot.tasks);
            }
            Some(_) => {}
            None if cs.peek().goals.has_value() => {
                let mut write = cs.write();
                write.goals.reset();
                write.tasks.reset();
            }
            None => {}
        }
    });

    // The selection is remembered in the same place Home remembers it, so switching tabs keeps you on the
    // board you were looking at rather than on whichever project sorts first. The PREFIX both times, which
    // is what the selection is — see `ComponentState::selected` on Home.
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
                h1 { class: "page-title", "Goals" }
            }
            div { class: "empty-note",
                "You are not on any project yet. Ask an admin to add you to one."
            }
        };
    }

    let selected_prefix = cs_ra.selected.clone();
    let show_closed = cs_ra.show_closed;
    let status_filter = cs_ra.status_filter.clone();

    let current = projects
        .iter()
        .find(|itm| itm.prefix == selected_prefix)
        .or_else(|| projects.first());

    let Some(current) = current else {
        return rsx! {
            div { class: "empty-note", "No project selected." }
        };
    };

    let header = rsx! {
        RenderHeader { projects: projects.to_vec(), cs }
    };

    let goals: Vec<GoalResponse> = match get_goals(cs, &cs_ra) {
        Ok(goals) => goals.to_vec(),
        Err(element) => {
            return rsx! {
                div { class: "goals-page",
                    {header}
                    {element}
                }
            };
        }
    };

    // The project's whole task list, archived work included — the same thing a push carries. A goal outlives
    // the archive window, so a list that stopped at it would disagree with the counters beside it.
    let tasks: Vec<TaskResponse> = match get_tasks(cs, &cs_ra) {
        Ok(tasks) => tasks.to_vec(),
        Err(element) => {
            return rsx! {
                div { class: "goals-page",
                    {header}
                    {element}
                }
            };
        }
    };

    // One pass, into the group each task belongs to. `goal` is the handle the server resolved, so a task
    // under a goal that is not on screen — a closed one, past its window — lands in neither group, which is
    // the same as not being drawn.
    let mut of_goal: HashMap<String, Vec<TaskResponse>> = HashMap::new();
    let mut loose: Vec<TaskResponse> = Vec::new();

    for task in &tasks {
        // A deleted task is not work this screen has anything to say about — it is not in the counters above
        // it either, and a list that disagreed with its own counter is the one thing this screen must not do.
        // Searching for it is Home's job; here it is simply gone.
        if task.deleted_unix_seconds.is_some() {
            continue;
        }

        match task.goal.as_ref() {
            Some(goal) => of_goal.entry(goal.clone()).or_default().push(task.clone()),
            None => loose.push(task.clone()),
        }
    }

    let wanted = GoalStatus::parse(&status_filter);

    // The other way of looking at the same goals: as bars over a month. It draws closed goals from any
    // month somebody scrolls back to, which the live list stops carrying once they age off — so this view,
    // and only this one, also reads the history.
    if cs_ra.view == GoalsView::Timeline {
        let archive: Vec<GoalResponse> = match get_archive(cs, &cs_ra) {
            Ok(archive) => archive.to_vec(),
            Err(element) => {
                return rsx! {
                    div { class: "goals-page",
                        {header}
                        {element}
                    }
                };
            }
        };

        // The same status the list's badge shows, and the same filter on it. In place of the list's tick,
        // Hide done — which also takes the finished tasks out from under an unfolded goal.
        let hide_done = cs_ra.hide_done;
        let on_timeline: Vec<(GoalResponse, GoalStatus)> = merge_with_archive(goals, &archive)
            .into_iter()
            .map(|goal| {
                let under = of_goal
                    .get(&goal.id)
                    .map(|itm| itm.as_slice())
                    .unwrap_or(&[]);
                let status = goal_status(&goal, under);
                (goal, status)
            })
            .filter(|(_, status)| match wanted {
                // A chosen status wins over Hide done, as it wins over the tick in the list.
                Some(wanted) => *status == wanted,
                None => !hide_done || *status != GoalStatus::Done,
            })
            .collect();

        let month = cs_ra.month;
        let expanded = cs_ra.expanded.clone();
        let current = current.clone();
        drop(cs_ra);

        return rsx! {
            div { class: "goals-page",
                {header}
                RenderTimeline {
                    goals: on_timeline,
                    of_goal,
                    project: current,
                    month,
                    wanted,
                    expanded,
                    hide_done,
                    on_month: move |month: Option<Month>| cs.write().show_month(month),
                    on_toggle: move |goal: String| cs.write().toggle(&goal),
                }
            }
        };
    }

    // Each goal's status, worked out ONCE and carried to the row that draws it. The filter below asks the
    // same question the badge answers, and two derivations of one fact is how the two come to disagree —
    // which is also why it happens here, after the tasks are grouped, rather than up where the goals
    // arrive: a goal's status is a statement about its tasks.

    let with_status: Vec<(GoalResponse, GoalStatus)> = goals
        .into_iter()
        .map(|goal| {
            let under = of_goal
                .get(&goal.id)
                .map(|itm| itm.as_slice())
                .unwrap_or(&[]);
            let status = goal_status(&goal, under);
            (goal, status)
        })
        .collect();

    // Whether the tick is what emptied the screen, asked BEFORE the filter runs — an empty board and a
    // board whose every goal is finished are two different things to be told, and afterwards they look
    // identical.
    let closed_hidden = !show_closed
        && wanted.is_none()
        && with_status
            .iter()
            .any(|(_, status)| *status == GoalStatus::Done);

    let of_this_status: Vec<(GoalResponse, GoalStatus)> = with_status
        .into_iter()
        .filter(|(_, status)| match wanted {
            // A status somebody CHOSE wins over the tick. Picking `Done` is the plainest way there is of
            // asking for the closed goals, and answering it with an empty screen because a checkbox
            // elsewhere is unticked would be obtuse — so the tick governs the unfiltered view only, and
            // says so by going dead while a status is chosen.
            Some(wanted) => *status == wanted,
            None => show_closed || *status != GoalStatus::Done,
        })
        .collect();

    // A goal belongs to no status the way a task belongs to no goal, so the Backlog is part of the
    // unfiltered picture only: somebody who asked for the `Todo` goals did not ask for the loose work.
    let show_backlog = wanted.is_none() && !loose.is_empty();

    // What to say when the list comes out empty — which now has three quite different reasons, and a screen
    // that said "nothing here yet" where a filter is what emptied it would be lying to the person holding
    // the filter.
    let empty_note: Option<String> = if !of_this_status.is_empty() || show_backlog {
        None
    } else if let Some(wanted) = wanted {
        Some(format!(
            "No goal on this board is {} right now.",
            wanted.title()
        ))
    } else if closed_hidden {
        Some("Every goal on this board is done. Tick \"Show done goals\" to see them.".to_string())
    } else {
        Some(
            "Nothing here yet. Goals are opened through MCP — ask an agent to open one."
                .to_string(),
        )
    };

    let current = current.clone();
    let expanded = cs_ra.expanded.clone();
    let picking_color = cs_ra.picking_color.clone();
    drop(cs_ra);

    rsx! {
        div { class: "goals-page",
            {header}

            if let Some(note) = empty_note {
                div { class: "empty-note", "{note}" }
            }

            div { class: "goals-list",
                for (goal, status) in of_this_status.iter() {
                    RenderGoal {
                        key: "{goal.id}",
                        goal: goal.clone(),
                        status: *status,
                        project: current.clone(),
                        open: expanded.contains(&goal.id),
                        tasks: of_goal.get(&goal.id).cloned().unwrap_or_default(),
                        picking_color: picking_color.as_deref() == Some(goal.id.as_str()),
                        cs,
                    }
                }

                // Last, and drawn as a goal without being one: work that belongs to no epic still has to be
                // visible, or this screen would quietly hide part of the board.
                if show_backlog {
                    RenderBacklog {
                        tasks: loose,
                        project: current.clone(),
                        open: expanded.iter().any(|itm| itm == BACKLOG),
                        cs,
                    }
                }
            }
        }
    }
}

/// The key the Backlog group is remembered under. Not a goal id — no goal can be called this, because a
/// handle always contains a `-`.
const BACKLOG: &str = "backlog";

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
                        // Matched against what actually came back, so a board somebody lost access to falls
                        // through to the first they can see rather than leaving the screen on nothing.
                        let remembered = crate::web::storage::get_last_project();

                        let initial = remembered
                            .filter(|prefix| {
                                response
                                    .projects
                                    .iter()
                                    .any(|itm| itm.prefix.eq_ignore_ascii_case(prefix))
                            })
                            .or_else(|| {
                                // A live board is preferred when nothing named one: a project somebody put
                                // away should not be what a fresh browser opens on. An archived one is still
                                // taken when every board is archived, since leaving the screen on nothing
                                // would be worse — and the picker keeps an option for whatever is open.
                                response
                                    .projects
                                    .iter()
                                    .find(|itm| !itm.archived)
                                    .or_else(|| response.projects.first())
                                    .map(|itm| itm.prefix.clone())
                            })
                            .unwrap_or_default();

                        let mut write = cs.write();
                        write.selected = initial;
                        write.projects.set_loaded(response.projects);
                        drop(write);

                        // The socket starts here for the same reason Home starts it there: nothing can be
                        // subscribed to until a project is known. It has to be started by THIS screen too —
                        // somebody who opens /goals directly would otherwise have no channel for changes at
                        // all, and every change arrives that way. Idempotent, so both doing it is fine.
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

fn get_goals(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[GoalResponse], Element> {
    if cs_ra.selected.is_empty() {
        return Ok(&[]);
    }

    match cs_ra.goals.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected.clone();

            spawn(async move {
                cs.write().goals.set_loading();

                // `false`: the live list. A goal past its window is not drawn here — see `get_goals`.
                match crate::api::get_goals(&project, false).await {
                    Ok(response) => cs.write().goals.set_loaded(response.goals),
                    Err(err) => cs.write().goals.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(goals) => Ok(goals.as_slice()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

/// The board's whole history of goals, for the timeline — see `ComponentState::archive`.
fn get_archive(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[GoalResponse], Element> {
    if cs_ra.selected.is_empty() {
        return Ok(&[]);
    }

    match cs_ra.archive.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected.clone();

            spawn(async move {
                cs.write().archive.set_loading();

                match crate::api::get_goals(&project, true).await {
                    Ok(response) => cs.write().archive.set_loaded(response.goals),
                    Err(err) => cs.write().archive.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(goals) => Ok(goals.as_slice()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

fn get_tasks(
    mut cs: Signal<ComponentState>,
    cs_ra: &ComponentState,
) -> Result<&[TaskResponse], Element> {
    if cs_ra.selected.is_empty() {
        return Ok(&[]);
    }

    match cs_ra.tasks.as_ref() {
        RenderState::None => {
            let project = cs_ra.selected.clone();

            spawn(async move {
                cs.write().tasks.set_loading();

                // `true`: everything, archived included. See the note where the list is grouped.
                match crate::api::get_tasks(&project, true).await {
                    Ok(response) => cs.write().tasks.set_loaded(response.tasks),
                    Err(err) => cs.write().tasks.set_error(err.message),
                }
            });

            Err(render_loading())
        }
        RenderState::Loading => Err(render_loading()),
        RenderState::Loaded(tasks) => Ok(tasks.as_slice()),
        RenderState::Error(err) => Err(render_error(err)),
    }
}

/// Where a goal has got to, in the words the board uses for a task: `Todo`, `In Progress`, `Done`.
///
/// **Derived here rather than read off the wire, because the wire has only two states.** A goal's `status`
/// is `close_moment` under another name — `GoalModel::status` returns `done` for a closed goal and `todo`
/// for every open one — so a goal with half its work landed reads exactly like one nobody has started. The
/// middle state is the one this screen is opened to see, and what answers it is already in hand: the tasks
/// grouped beside the goal.
///
/// The server's counter is consulted FIRST and the list only after, and that order is what keeps this from
/// contradicting the `done / total` beside it: `done_amount` counts ARCHIVED work, so a goal whose early
/// tasks have all aged off the board still says it has started.
fn goal_status(goal: &GoalResponse, tasks: &[TaskResponse]) -> GoalStatus {
    if goal.closed_unix_seconds.is_some() {
        return GoalStatus::Done;
    }

    // Somebody saying it has started is the plainest evidence there is, and it may come before any of its
    // tasks moves.
    let started = goal.started_unix_seconds.is_some()
        || goal.done_amount > 0
        || tasks.iter().any(|task| task.status != COLUMN_ID_TODO);

    if started {
        GoalStatus::InProgress
    } else {
        GoalStatus::Todo
    }
}

/// A goal and the status its row shows, found among a board's goals by the goal's id.
///
/// For a screen that names a goal without holding it: a release carries the id, the name and the colour of
/// the goal it shipped and nothing else, while the dialog wants the whole goal and a status beside it. The
/// status is worked out HERE — by [`goal_status`], over the tasks picked the way the list above groups
/// them — rather than by whoever asks, because a dialog that said `Todo` where this screen says
/// `In Progress` would be two answers to one question.
///
/// `None` when no goal on the list has that id. Whether that means "archived" or "gone" is the caller's to
/// know: it is the one that chose which list to pass.
pub fn find_goal_with_status(
    goals: &[GoalResponse],
    tasks: &[TaskResponse],
    goal_id: &str,
) -> Option<(GoalResponse, &'static str)> {
    let goal = goals.iter().find(|itm| itm.id == goal_id)?;

    // The two conditions the grouping in `RenderGoals` applies: under this goal, and not deleted.
    let under: Vec<TaskResponse> = tasks
        .iter()
        .filter(|task| task.deleted_unix_seconds.is_none() && task.goal.as_deref() == Some(goal_id))
        .cloned()
        .collect();

    Some((goal.clone(), goal_status(goal, &under).title()))
}

/// The three states [`goal_status`] can report, and how each is drawn.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum GoalStatus {
    Todo,
    InProgress,
    Done,
}

impl GoalStatus {
    /// In the order the work moves through them, which is the order the filter offers them in.
    const ALL: [GoalStatus; 3] = [Self::Todo, Self::InProgress, Self::Done];

    /// Back from a [`Self::key`]. Anything else is "any status" — an empty box and a filter naming
    /// something that no longer exists are the same screen, and neither is worth an error.
    pub(super) fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|itm| itm.key() == key)
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Todo => "Todo",
            Self::InProgress => "In Progress",
            Self::Done => "Done",
        }
    }

    /// The one name this status is known by outside its own type: the modifier beside `goal-status` in
    /// the stylesheet, and the value of the option in the header's filter. One vocabulary, so a colour and
    /// a filter cannot drift apart.
    pub(super) fn key(self) -> &'static str {
        match self {
            Self::Todo => "todo",
            Self::InProgress => "progress",
            Self::Done => "done",
        }
    }

    /// What the badge says on hover, which is the half a one-word label cannot carry.
    pub(super) fn hint(self) -> &'static str {
        match self {
            Self::Todo => "Nothing under this goal has been started yet",
            Self::InProgress => "Work under this goal has started",
            Self::Done => "Closed — every task under it landed",
        }
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
fn RenderHeader(projects: Vec<ProjectResponse>, cs: Signal<ComponentState>) -> Element {
    let cs_ra = cs.read();
    let selected_prefix = cs_ra.selected.clone();
    let show_closed = cs_ra.show_closed;
    let status_filter = cs_ra.status_filter.clone();
    let view = cs_ra.view;
    let hide_done = cs_ra.hide_done;
    drop(cs_ra);

    // While a status is chosen the tick has nothing left to govern — that choice already decides whether
    // the closed goals are on screen. Drawn dead rather than removed: a control that vanishes when you use
    // the one beside it is a control people stop trusting.
    let filtered = GoalStatus::parse(&status_filter).is_some();
    let on_timeline = view == GoalsView::Timeline;

    let mut cs = cs;

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Goals" }
            div { class: "project-picker",
                // `selected` on the option rather than `value` on the select, or a remembered project is
                // restored into the state and not shown in the control.
                select {
                    onchange: move |event| cs.write().select(event.value()),
                    // Archived boards are left out, except the one currently open — which by the comment
                    // above is exactly the case that would otherwise display the wrong project, since the
                    // control follows `selected` and an open board with no option has none to follow.
                    for project in projects.iter().filter(|itm| !itm.archived || itm.prefix == selected_prefix) {
                        option {
                            value: "{project.prefix}",
                            selected: project.prefix == selected_prefix,
                            "{project.prefix} · {project.name}"
                        }
                    }
                }
                // Straight after the board, because it narrows the board: which goals of it are on
                // screen. Empty is every one of them, spelled out as an option rather than left as the
                // blank the other pickers on Home use — a filter whose "off" state has no name is a filter
                // people are not sure they have turned off.
                select {
                    onchange: move |event| cs.write().status_filter = event.value(),
                    option { value: "", selected: status_filter.is_empty(), "Any status" }
                    for status in GoalStatus::ALL.iter() {
                        option {
                            value: "{status.key()}",
                            selected: status.key() == status_filter,
                            "{status.title()}"
                        }
                    }
                }
                // Beside the picker rather than out at the right edge: it says which goals of this board are
                // on screen, which is the same question the dropdown answers one level up.
                //
                // The timeline has its own answer in the same place: it opens on everything a month held,
                // done work included — it is the view somebody reads the past in — and Hide done is how
                // they put the finished goals and tasks away. Kept apart from the list's tick, whose default
                // is the opposite.
                if on_timeline {
                    button {
                        class: if hide_done { "btn btn-sm toggle active" } else { "btn btn-sm toggle" },
                        title: if hide_done { "Done goals and tasks are hidden — click to show them" } else { "Hide the goals and the tasks that are done" },
                        onclick: move |_| cs.write().toggle_hide_done(),
                        "Hide done"
                    }
                } else {
                    div {
                        class: if filtered { "checkbox-row disabled" } else { "checkbox-row" },
                        title: if filtered { "A chosen status already decides this" } else { "" },
                        input {
                            r#type: "checkbox",
                            id: "goals-show-closed",
                            disabled: filtered,
                            checked: show_closed,
                            onchange: move |event| cs.write().show_closed = event.checked(),
                        }
                        label { r#for: "goals-show-closed", "Show done goals" }
                    }
                }
            }
            // At the far edge, away from the filters: it does not narrow what is shown, it changes how.
            div { class: "view-switch",
                for option in [GoalsView::List, GoalsView::Timeline] {
                    button {
                        key: "{option.key()}",
                        class: if option == view { "active" } else { "" },
                        onclick: move |_| cs.write().set_view(option),
                        match option {
                            GoalsView::List => "List",
                            GoalsView::Timeline => "Timeline",
                        }
                    }
                }
            }
        }
    }
}

/// One goal, folded or open.
#[component]
fn RenderGoal(
    goal: GoalResponse,
    status: GoalStatus,
    project: ProjectResponse,
    open: bool,
    tasks: Vec<TaskResponse>,
    picking_color: bool,
    cs: Signal<ComponentState>,
) -> Element {
    let mut cs = cs;

    let closed = goal.closed_unix_seconds.is_some();
    let hex = KindColor::parse_or_default(&goal.color).hex();
    let priority = task_manager_shared::priority::Priority::parse_or_default(&goal.priority);

    // Counters as the server sent them, and NOT recomputed from the list below even though that list is
    // now in hand for every row: the numbers count archived work, and a figure counted from a board read
    // would report finished work as half-done. The status badge beside them is allowed to consult the list
    // because it asks a boolean rather than a number — see `goal_status`.
    let done = goal.done_amount;
    let total = goal.tasks_amount;

    let percent = if total > 0 { done * 100 / total } else { 0 };

    let goal_id = goal.id.clone();

    let for_dialog = goal.clone();
    let swatch_goal = goal.id.clone();
    let picker_project = project.prefix.clone();
    let picker_goal = goal.id.clone();

    rsx! {
        div {
            class: if closed { "goal-card closed" } else { "goal-card" },
            // The goal's colour on its own edge as well as on its cards, so the two are recognisably the
            // same goal when the screens sit side by side.
            style: "border-left: 3px solid {hex}",

            div {
                class: "goal-head",
                // Fetching from the handler, not from the render body: opening a goal IS the moment its
                // work is wanted, and a write to a signal during render is the one thing the design
                // patterns forbid. A list already in hand is not fetched again — a push clears it, which is
                // what makes the next expansion ask.
                onclick: move |_| cs.write().toggle(&goal_id),

                span { class: "goal-caret", if open { "▾" } else { "▸" } }
                img { class: "goal-icon", src: asset!("/public/assets/images/goal.svg"), alt: "" }

                // `stop_propagation` because the whole head is the fold/unfold target: without it, changing
                // a colour would also open or close the goal, which is two things for one click.
                button {
                    class: "goal-swatch",
                    style: "background: {hex}",
                    title: "Colour this goal",
                    onclick: move |event| {
                        event.stop_propagation();

                        let mut write = cs.write();
                        write.picking_color = if write.picking_color.as_deref() == Some(swatch_goal.as_str()) {
                            None
                        } else {
                            Some(swatch_goal.clone())
                        };
                    },
                }

                div { class: "goal-head-text",
                    // Before the name rather than out in the meta, and the board is why: a task sticker
                    // already reads `TM-G7` then the goal's title, in that order. A handle that sat at the
                    // far right here would be the same identifier in two different places depending on
                    // which screen you were looking at — and this is the one people copy into `goal` on a
                    // tasks_create call, so it wants to be where the eye already is.
                    span { class: "goal-id", "{goal.id}" }
                    div { class: "goal-name", "{goal.name}" }
                }

                div { class: "goal-meta",
                    // Only when it was ranked, exactly as on a card: this screen is already in priority order,
                    // so the badge is here to say why a goal is where it is rather than to label every row.
                    if priority.is_worth_showing() {
                        span {
                            class: "sticker-priority",
                            style: "background: {priority.hex()}",
                            title: "Priority: {priority.title()}",
                            "{priority.title()}"
                        }
                    }
                    // Beside the priority, and always drawn — unlike the priority, which only appears
                    // when somebody ranked it. Where a goal has got to is the question this screen exists
                    // to answer, so the one row that has no badge for it is the row that raises it.
                    //
                    // This is also the `Closed` flag that used to sit here: `Done` says the same thing in
                    // the vocabulary the tasks under it are labelled with, and two badges for one fact is
                    // one badge too many.
                    span {
                        class: "goal-status {status.key()}",
                        title: "{status.hint()}",
                        "{status.title()}"
                    }
                    span { class: "goal-progress-text", "{done} / {total}" }
                    div { class: "goal-progress",
                        div { class: "goal-progress-fill", style: "width: {percent}%" }
                    }
                    // Before the thread count, because the two answer different halves of "is there
                    // anything to read before I start": a document is what the work is done AGAINST, a
                    // comment is what somebody SAID about it. Drawn only when there is one — an epic
                    // with neither would otherwise carry two zeroes down every row.
                    if !goal.documents.is_empty() {
                        span {
                            class: "goal-documents",
                            title: "{goal.documents.len()} document(s) attached — open the goal to read them",
                            "📄 {goal.documents.len()}"
                        }
                    }
                    // The third of the same kind of hint, and the one that says where the goal got to
                    // AFTER the work: the counters beside it stop at `done`, and whether what was done is
                    // out is a different fact. Drawn only when there is one, like its two neighbours.
                    if !goal.releases.is_empty() {
                        span {
                            class: "goal-releases",
                            title: "Went out in {goal.releases.len()} release(s) — open the goal to read them",
                            "🚀 {goal.releases.len()}"
                        }
                    }
                    if goal.comments.len() > 0 {
                        span { class: "goal-comments", title: "{goal.comments.len()} notes on the thread",
                            "💬 {goal.comments.len()}"
                        }
                    }

                    // The description and the thread live behind this, not in the head: a head that carried
                    // the text would either truncate it — which reads as a broken sentence — or make every
                    // row a different height. `stop_propagation` for the same reason as the swatch: the head
                    // itself folds the goal, and one click must do one thing.
                    button {
                        class: "sticker-view",
                        title: "Read this goal and its comments",
                        onclick: move |event| {
                            event.stop_propagation();
                            crate::dialogs::open(crate::dialogs::DialogState::ViewGoal {
                                goal: for_dialog.clone(),
                                // Handed over rather than worked out again in there: the dialog holds the
                                // goal but not the tasks the middle state is read from, and a dialog that
                                // said `Open` where the row behind it says `In Progress` is two answers to
                                // one question.
                                status: status.title().to_string(),
                            });
                        },
                        "👁"
                    }
                }
            }

            if picking_color {
                div {
                    class: "goal-palette",
                    onclick: move |event| event.stop_propagation(),
                    crate::dialogs::RenderColorPicker {
                        value: goal.color.clone(),
                        on_pick: move |color: String| {
                            let project = picker_project.clone();
                            let goal_id = picker_goal.clone();

                            // Closed first, then the request: the answer comes back as a WebSocket push
                            // carrying the whole board, so there is nothing here to wait for and nothing to
                            // write back by hand.
                            cs.write().picking_color = None;

                            spawn(async move {
                                if let Err(err) = crate::api::set_goal_color(&project, &goal_id, &color).await {
                                    crate::web::console_log(
                                        format!("recolouring {goal_id} failed: {}", err.message).as_str(),
                                    );
                                }
                            });
                        },
                    }
                }
            }

            if open {
                div { class: "goal-body",
                    // In hand, not fetched: the list came with the goal in the same snapshot, so there is no
                    // loading state to draw and no failure to report.
                    if tasks.is_empty() {
                        div { class: "empty-note",
                            "No tasks under this goal yet — it is still being talked about."
                        }
                    } else {
                        for task in tasks.iter() {
                            RenderGoalTask { key: "{task.id}", task: task.clone(), project: project.clone() }
                        }
                    }
                }
            }
        }
    }
}

/// Tasks that belong to no goal, drawn like a goal so nothing on the board is invisible from this screen.
///
/// Not in the database and never will be: it is a group, not a container. Live work only — loose tasks are
/// nobody's epic, so there is no history of them to show.
#[component]
fn RenderBacklog(
    tasks: Vec<TaskResponse>,
    project: ProjectResponse,
    open: bool,
    cs: Signal<ComponentState>,
) -> Element {
    let mut cs = cs;

    let done = tasks
        .iter()
        .filter(|task| task.status == COLUMN_ID_DONE)
        .count();

    rsx! {
        div { class: "goal-card backlog",
            div {
                class: "goal-head",
                onclick: move |_| cs.write().toggle(BACKLOG),

                span { class: "goal-caret", if open { "▾" } else { "▸" } }

                div { class: "goal-head-text",
                    div { class: "goal-name", "Backlog" }
                    div { class: "goal-note", "not part of any goal" }
                }

                div { class: "goal-meta",
                    span { class: "goal-progress-text", "{done} / {tasks.len()}" }
                }
            }

            if open {
                div { class: "goal-body",
                    for task in tasks.iter() {
                        RenderGoalTask { key: "{task.id}", task: task.clone(), project: project.clone() }
                    }
                }
            }
        }
    }
}

/// One task under a goal: a line rather than a card.
///
/// A line, because what this screen is for is the shape of a goal — twenty cards would bury it. What is on
/// the line is what a person asks about a task without opening it: what kind of work it is, where it sits,
/// whether anybody has said anything, and who has it. The whole task is one click away, in the same dialog
/// the board opens.
#[component]
fn RenderGoalTask(task: TaskResponse, project: ProjectResponse) -> Element {
    let title = task_manager_shared::task_title::task_title(&task.text);
    let priority = task_manager_shared::priority::Priority::parse_or_default(&task.priority);

    let kind = task
        .kind
        .as_ref()
        .and_then(|kind_id| project.kinds.iter().find(|itm| &itm.id == kind_id));

    let kind_hex = kind
        .map(|itm| KindColor::parse_or_default(&itm.color).hex())
        .unwrap_or("");

    let status_name = task_status_name(&task.status, &project);

    let done = task.status == COLUMN_ID_DONE;

    let assignee = task
        .assignee_name
        .clone()
        .or_else(|| task.assignee.clone());

    let found = crate::api::find_task_locally(&task, &project);

    rsx! {
        div {
            class: if done { "goal-task done" } else { "goal-task" },
            onclick: move |_| {
                crate::dialogs::open(crate::dialogs::DialogState::ViewTask { found: found.clone() });
            },

            span { class: "goal-task-id", "{task.id}" }

            // Same rule as on a card: drawn only when it was ranked, because the row's position already says
            // it and a label on every line says nothing.
            if priority.is_worth_showing() {
                span {
                    class: "sticker-priority",
                    style: "background: {priority.hex()}",
                    title: "Priority: {priority.title()}",
                    "{priority.title()}"
                }
            }

            if let Some(kind) = kind {
                span {
                    class: "goal-task-kind",
                    style: "background: {kind_hex}",
                    title: "{kind.description}",
                    if crate::web::icon_exists(&kind.icon) {
                        img { class: "sticker-kind-icon", src: "{crate::web::icon_url(&kind.icon)}", alt: "" }
                    }
                    "{kind.name}"
                }
            }

            span { class: "goal-task-title", "{title}" }

            if task.blocked {
                span { class: "sticker-blocked-flag", "Blocked" }
            }

            // Only when there is a thread. A `0` on every line is noise that makes the lines that do have
            // something harder to spot.
            if !task.comments.is_empty() {
                span { class: "goal-task-comments", title: "{task.comments.len()} comments",
                    "💬 {task.comments.len()}"
                }
            }

            span {
                class: if assignee.is_some() { "goal-task-assignee" } else { "goal-task-assignee unassigned" },
                {assignee.clone().unwrap_or_else(|| "Unassigned".to_string())}
            }

            span { class: "goal-task-status", "{status_name}" }
        }
    }
}

/// The name of the column a task is in, as the board shows it.
///
/// Both anchors exist in every project but are not in `columns`, which holds the middle only — so a task in
/// Todo or Done would otherwise show a raw id where every other row shows a name.
pub(super) fn task_status_name(status: &str, project: &ProjectResponse) -> String {
    match status {
        COLUMN_ID_TODO => "Todo".to_string(),
        COLUMN_ID_DONE => "Done".to_string(),
        stored => project
            .columns
            .iter()
            .find(|column| column.id == stored)
            .map(|column| column.name.clone())
            .unwrap_or_else(|| stored.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(id: &str) -> GoalResponse {
        GoalResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            name: id.to_string(),
            description: String::new(),
            color: String::new(),
            priority: String::new(),
            status: "todo".to_string(),
            tasks_amount: 0,
            done_amount: 0,
            documents: Vec::new(),
            subtasks: Vec::new(),
            releases: Vec::new(),
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            started_unix_seconds: None,
            closed_unix_seconds: None,
            deleted_unix_seconds: None,
        }
    }

    fn task(id: &str, goal: &str, status: &str) -> TaskResponse {
        TaskResponse {
            id: id.to_string(),
            project: "RMS".to_string(),
            text: id.to_string(),
            status: status.to_string(),
            priority: String::new(),
            kind: None,
            goal: Some(goal.to_string()),
            goal_name: None,
            goal_color: None,
            assignee: None,
            assignee_name: None,
            labels: Vec::new(),
            depends_on: Vec::new(),
            blocks: Vec::new(),
            link_statuses: Vec::new(),
            blocked: false,
            documents: Vec::new(),
            subtasks: Vec::new(),
            gh_actions: Vec::new(),
            comments: Vec::new(),
            created_unix_seconds: 0,
            updated_unix_seconds: 0,
            started_unix_seconds: None,
            closed_unix_seconds: None,
            deleted_unix_seconds: None,
        }
    }

    /// The Releases screen opens a goal by its id and has to say about it what this screen's row says —
    /// so the three answers are checked through the one function both go through.
    #[test]
    fn a_goal_found_by_id_carries_the_status_its_row_shows() {
        let mut shipped = goal("RMS-G3");
        shipped.closed_unix_seconds = Some(1_791_331_200);

        let goals = [goal("RMS-G1"), goal("RMS-G2"), shipped];

        let tasks = [
            task("RMS-1", "RMS-G1", COLUMN_ID_TODO),
            task("RMS-2", "RMS-G2", COLUMN_ID_TODO),
            task("RMS-3", "RMS-G2", "in-progress"),
        ];

        let status_of = |id: &str| find_goal_with_status(&goals, &tasks, id).map(|(_, status)| status);

        assert_eq!(status_of("RMS-G1"), Some("Todo"));
        assert_eq!(
            status_of("RMS-G2"),
            Some("In Progress"),
            "one task off the first column is a goal that has started"
        );
        assert_eq!(status_of("RMS-G3"), Some("Done"), "closed is done, whatever is under it");

        let (found, _) = find_goal_with_status(&goals, &tasks, "RMS-G2").unwrap();
        assert_eq!(found.id, "RMS-G2", "the goal that was asked for, whole");
    }

    /// Only this goal's own live work speaks for it: a neighbour's task that has started, and a task of
    /// its own that was deleted after it started, are the two ways to be told `In Progress` wrongly.
    #[test]
    fn another_goals_work_and_deleted_work_do_not_start_a_goal() {
        let goals = [goal("RMS-G1"), goal("RMS-G2")];

        let mut deleted = task("RMS-2", "RMS-G1", "in-progress");
        deleted.deleted_unix_seconds = Some(1_791_331_200);

        let tasks = [
            task("RMS-1", "RMS-G1", COLUMN_ID_TODO),
            deleted,
            task("RMS-3", "RMS-G2", "in-progress"),
        ];

        let (_, status) = find_goal_with_status(&goals, &tasks, "RMS-G1").unwrap();
        assert_eq!(status, "Todo");
    }

    /// A goal whose finished work has aged off the board has no started task to point at — and has
    /// started all the same, which is what the server's counter is consulted for.
    #[test]
    fn archived_done_work_still_means_the_goal_has_started() {
        let mut half_way = goal("RMS-G1");
        half_way.tasks_amount = 4;
        half_way.done_amount = 2;

        let (_, status) = find_goal_with_status(&[half_way], &[], "RMS-G1").unwrap();
        assert_eq!(status, "In Progress");
    }

    /// The live list does not hold a goal closed past its window, and a release is exactly the thing that
    /// outlives its goal — so "not here" has to be an answer the caller can act on, not a panic or a
    /// stand-in goal.
    #[test]
    fn a_goal_that_is_not_on_the_list_is_not_found() {
        let goals = [goal("RMS-G1")];

        assert!(find_goal_with_status(&goals, &[], "RMS-G7").is_none());
        assert!(find_goal_with_status(&[], &[], "RMS-G1").is_none());
    }
}
