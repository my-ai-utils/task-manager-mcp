use dioxus::prelude::*;
use task_manager_shared::kind_color::KindColor;
use task_manager_shared::priority::Priority;
use task_manager_shared::projects::{COLUMN_ID_DONE, COLUMN_ID_TODO, ProjectResponse};
use task_manager_shared::tasks::TaskResponse;

use dioxus_utils::{DataState, RenderState};

use crate::states::AppState;

/// The board.
///
/// **Read-only, deliberately.** Nothing here is dragged, clicked to edit or double-clicked: every change
/// to a task arrives through `/mcp`. What this screen owes the reader is an accurate picture, which is
/// why it repaints from a WebSocket push rather than hoping somebody reloads.
#[component]
pub fn RenderHome(search: Option<String>) -> Element {
    let app_state = consume_context::<Signal<AppState>>();

    let seeded = search.clone().unwrap_or_default();

    // The URL seeds the box, and from there the box is what the screen reads: every keystroke replaces the
    // URL, not the other way round. That direction matters — the router's query parser splits on `&` AFTER
    // percent-decoding, so a search containing one cannot survive a round trip, and a lossy address bar must
    // not cost the filter its text.
    let mut cs = use_signal({
        let seeded = seeded.clone();
        move || ComponentState::with_search(seeded)
    });

    // The other direction, for the times the URL changes from OUTSIDE this box: the Home tab points at a
    // board with nothing searched, and Back can land on a different `?search=`. Adopting a URL that has got
    // out of step is what stops the address bar and the box telling two different stories.
    //
    // `use_reactive!` because a prop is not a signal — a plain `use_effect` closes over the first render's
    // value and never sees another. Compared trimmed, so a space being typed at the end of a word is not a
    // disagreement and cannot snatch the caret back.
    use_effect(use_reactive!(|search| {
        let from_url = search.unwrap_or_default();

        let projects = {
            let cs_ra = cs.peek();

            if from_url.trim() == cs_ra.search.trim() {
                return;
            }

            match cs_ra.projects.as_ref() {
                RenderState::Loaded(projects) => projects.clone(),
                _ => Vec::new(),
            }
        };

        set_search(&mut cs.write(), from_url, &projects);
    }));

    // A link that names a task opens it. From the URL only, and only at mount: this is what somebody handed
    // `/?search=RMS-42` came for. The lookup goes through the server because it is the only side that can
    // turn a handle into a task — it knows which prefixes exist and which have moved — and `not_found` is
    // left closed, since "nothing there" is not worth a dialog nobody asked for.
    use_future(move || {
        let seeded = seeded.clone();

        async move {
            if !looks_like_a_task_id(seeded.trim()) {
                return;
            }

            if let Ok(found) = crate::api::find_task(seeded.trim()).await {
                if found.task.is_some() {
                    crate::dialogs::open(crate::dialogs::DialogState::ViewTask { found });
                }
            }
        }
    });

    let cs_ra = cs.read();

    // A WebSocket push arrives carrying the board. Applying it is a `set_loaded` — no request, no `Loading…`,
    // and nothing on screen is emptied before the new cards are there, which is what stops the board
    // flinching every time an agent touches a task.
    //
    // An effect rather than a read inside the loader because `use_effect` IS reactive where `use_future` is
    // not, and reacting to a push is the whole job.
    use_effect(move || {
        let app_ra = app_state.read();
        // Read even though the snapshot below is what gets used: it changes on every push, so it is what
        // makes two pushes carrying identical tasks count as two.
        let _revision = app_ra.board_revision;
        let push = app_ra.board_push.clone();
        drop(app_ra);

        match push {
            // Only the board on screen. A push for the project somebody just switched away from can still be
            // in flight, and applying it would put the previous board back under the new project's name.
            Some(snapshot) if snapshot.project == cs.peek().selected => {
                cs.write().tasks.set_loaded(snapshot.tasks);
            }
            Some(_) => {}
            // A push with no board: re-read, which is what this screen did before snapshots existed. Only
            // when something is already loaded — otherwise the loader below has it in hand anyway.
            None if cs.peek().tasks.has_value() => {
                cs.write().tasks.reset();
            }
            None => {}
        }
    });

    // Side effects of a selection, in an effect because that is what `use_effect` is for and because it IS
    // reactive: remember the choice for the next visit, and tell the socket which board to push about.
    // Sending the subscription on every change is also what makes switching projects not need a reconnect.
    //
    // Both take the PREFIX, which is what the selection now is — there is nothing left to translate here.
    // This effect used to look the project up in its own list to turn an id back into a prefix for storage,
    // and that translation is the thing that went wrong.
    use_effect(move || {
        let prefix = cs.read().selected.clone();

        if !prefix.is_empty() {
            crate::web::storage::save_last_project(&prefix);
            crate::web::watch_project(&prefix);
        }
    });

    // Loaded first, and the one thing the rest of the screen cannot do without. `Err` carries what to show
    // instead — a spinner or the failure — so the body below stays a straight line.
    let projects = match get_projects(cs, &cs_ra) {
        Ok(projects) => projects,
        Err(element) => return element,
    };

    if projects.is_empty() {
        return rsx! {
            div { class: "page-header",
                h1 { class: "page-title", "Home" }
            }
            div { class: "empty-note",
                "You are not on any project yet. Ask an admin to add you to one."
            }
        };
    }

    let selected_prefix = cs_ra.selected.clone();

    let current = projects
        .iter()
        .find(|itm| itm.prefix == selected_prefix)
        .or_else(|| projects.first());

    let Some(current) = current else {
        return rsx! {
            div { class: "empty-note", "No project selected." }
        };
    };

    // Keyed on the selection: picking another board resets this, which is what makes the next render load
    // it. See `ComponentState::select`.
    //
    // While it loads the header stays up — hiding the project picker mid-switch makes the screen jump.
    let tasks_ra: Vec<TaskResponse> = match get_tasks(cs, &cs_ra) {
        Ok(tasks) => tasks.to_vec(),
        Err(element) => {
            return rsx! {
                div { class: "board-page",
                    RenderHeader {
                        projects: projects.to_vec(),
                        current: current.clone(),
                        cs,
                        assignees: Vec::new(),
                        goals: Vec::new(),
                    }
                    {element}
                }
            };
        }
    };

    let search_text = cs_ra.search.clone();
    let kind_wanted = cs_ra.kind_filter.clone();
    let assignee_wanted = cs_ra.assignee_filter.clone();
    let goal_wanted = cs_ra.goal_filter.clone();

    // Every filter narrows the same list, so they compose: a type AND an assignee AND whatever is in the box.
    //
    // The archive filter is one of them now, and it is not a preference: a pushed snapshot carries the
    // project's WHOLE history — the Goals screen needs it — so this screen has to draw the live window
    // itself. The rule is `is_task_archived` in the shared crate, the same one the server applies to the
    // REST read, which is what keeps the two doors showing the same board.
    let now_unix_seconds = js_sys::Date::now() as i64 / 1_000;

    let visible: Vec<TaskResponse> = tasks_ra
        .iter()
        .filter(|task| {
            !task_manager_shared::tasks::is_task_archived(
                task,
                current.archive_days,
                now_unix_seconds,
            )
        })
        // Deleted work is hidden — UNLESS something is being searched for, which is the one moment anybody
        // wants it. That is why the snapshot carries it at all: a deletion that left nothing behind would
        // answer a search for its id with "no such task", indistinguishable from a typo.
        .filter(|task| task.deleted_unix_seconds.is_none() || !search_text.trim().is_empty())
        .filter(|task| matches_search(task, &search_text))
        .filter(|task| matches_kind(task, &kind_wanted))
        .filter(|task| matches_assignee(task, &assignee_wanted))
        .filter(|task| matches_goal(task, &goal_wanted))
        .cloned()
        .collect();

    // Offered from what is actually ON the board rather than from the roster: an option that matches
    // nothing is a dead end, and the whole point of the list is to narrow to something. Which is why it is
    // built from the live list and not from `tasks_ra` — somebody whose only work here is archived is not
    // on this board.
    let live: Vec<TaskResponse> = tasks_ra
        .iter()
        .filter(|task| task.deleted_unix_seconds.is_none())
        .filter(|task| {
            !task_manager_shared::tasks::is_task_archived(
                task,
                current.archive_days,
                now_unix_seconds,
            )
        })
        .cloned()
        .collect();

    let assignees = assignees_on_board(&live);
    let goals = goals_on_board(&live);

    // A flex column filling what is left of the window, so the board below it can be full height and each
    // of its columns can scroll on its own.
    rsx! {
        div { class: "board-page",
            RenderHeader {
                projects: projects.to_vec(),
                current: current.clone(),
                cs,
                assignees,
                goals,
            }

            if !current.description.trim().is_empty() {
                p { class: "page-note", "{current.description}" }
            }

            div { class: "board",
                for column in board_columns(current) {
                    RenderColumn {
                        key: "{column.id}",
                        column: column.clone(),
                        project: current.clone(),
                        tasks: visible.clone(),
                        cs,
                    }
                }
            }
        }
    }
}

/// The filter value standing for "nobody is on it".
///
/// A sentinel rather than an empty string, because empty already means "anyone" — and "unassigned" is a
/// thing you genuinely want to filter for: it is the pile nobody has picked up.
const UNASSIGNED: &str = "\u{0}unassigned";

/// The filter value meaning "work that belongs to no goal".
///
/// A NUL for the same reason `UNASSIGNED` carries one: it has to be a value no real goal handle can be, and a
/// handle is `RMS-G7` — letters, digits and a dash. Nothing a person can type collides with it.
const NO_GOAL: &str = "\u{0}nogoal";

fn matches_kind(task: &TaskResponse, wanted: &str) -> bool {
    if wanted.is_empty() {
        return true;
    }

    task.kind.as_deref() == Some(wanted)
}

/// Whether a task belongs to the goal being filtered for. An empty filter matches everything, and the one
/// reserved value matches work under NO goal — the same shape the assignee filter uses for "unassigned",
/// because "show me what is not in any epic" is exactly as useful a question.
fn matches_goal(task: &TaskResponse, wanted: &str) -> bool {
    if wanted.is_empty() {
        return true;
    }

    if wanted == NO_GOAL {
        return task.goal.is_none();
    }

    task.goal.as_deref() == Some(wanted)
}

fn matches_assignee(task: &TaskResponse, wanted: &str) -> bool {
    if wanted.is_empty() {
        return true;
    }

    match task.assignee.as_deref() {
        None => wanted == UNASSIGNED,
        // Case-insensitive: an address is stored lower-cased and the reserved `AI` is stored as declared,
        // so comparing exactly would depend on which of the two this happens to be.
        Some(assignee) => wanted != UNASSIGNED && assignee.eq_ignore_ascii_case(wanted),
    }
}

/// Everyone with something on this board, as (value, label), sorted by label.
///
/// Deduplicated case-insensitively and labelled with the display name where there is one — the value has to
/// be what is stored, but nobody picks a colleague out of a list of email addresses.
fn assignees_on_board(tasks: &[TaskResponse]) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    for task in tasks {
        let Some(assignee) = task.assignee.as_deref() else {
            continue;
        };

        if result
            .iter()
            .any(|(value, _)| value.eq_ignore_ascii_case(assignee))
        {
            continue;
        }

        let label = task
            .assignee_name
            .clone()
            .filter(|itm| !itm.trim().is_empty())
            .unwrap_or_else(|| assignee.to_string());

        result.push((assignee.to_string(), label));
    }

    result.sort_by_key(|itm| itm.1.to_lowercase());
    result
}

/// The goals that actually have work on this board, as `(handle, label)`.
///
/// Built from the tasks rather than from a goals list, and for the same reason the assignee options are: an
/// option that matches nothing is a dead end, and the point of the control is to narrow to something. It also
/// means this screen needs no second request — a task carries its goal's handle and name already.
fn goals_on_board(tasks: &[TaskResponse]) -> Vec<(String, String)> {
    let mut result: Vec<(String, String)> = Vec::new();

    for task in tasks {
        let Some(goal) = task.goal.as_deref() else {
            continue;
        };

        if result.iter().any(|(value, _)| value == goal) {
            continue;
        }

        let label = task
            .goal_name
            .clone()
            .filter(|itm| !itm.trim().is_empty())
            .map(|name| format!("{goal} · {name}"))
            .unwrap_or_else(|| goal.to_string());

        result.push((goal.to_string(), label));
    }

    result.sort_by_key(|itm| itm.1.to_lowercase());
    result
}

/// A task handle split into the prefix that names a board and the digits that name a task on it.
///
/// A shape test, not a parse: `PREFIX-42`. The server does the authoritative parse — it is the only side
/// that knows which prefixes exist and which have moved — so all this decides is which of the two things
/// to do with what was typed.
fn task_id_parts(query: &str) -> Option<(&str, &str)> {
    let (prefix, number) = query.rsplit_once('-')?;

    let ok = !prefix.is_empty()
        && prefix
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !number.is_empty()
        && number.chars().all(|c| c.is_ascii_digit());

    if ok { Some((prefix, number)) } else { None }
}

/// Whether this looks like a task handle rather than something to search for.
fn looks_like_a_task_id(query: &str) -> bool {
    task_id_parts(query).is_some()
}

/// The project a prefix names: the one holding it NOW first, and only then one that used to.
///
/// The order is the whole point. A prefix can be renamed away and then taken by a different project, so
/// history that outranked the present would send `RMS-42` to the board that used to be RMS rather than the
/// one that is.
fn project_by_prefix<'a>(
    projects: &'a [ProjectResponse],
    prefix: &str,
) -> Option<&'a ProjectResponse> {
    projects
        .iter()
        .find(|itm| itm.prefix.eq_ignore_ascii_case(prefix))
        .or_else(|| {
            projects.iter().find(|itm| {
                itm.prefix_history
                    .iter()
                    .any(|old| old.eq_ignore_ascii_case(prefix))
            })
        })
}

/// Which board to open on: the one a handle in the box names, then the one this browser was last on, then
/// the first there is.
///
/// A link carrying a handle beats the remembered board, because the handle is what the link is for.
///
/// **`remembered` is a PREFIX, and it is resolved as one.** This is where the reload bug lived: storage keeps
/// what a person calls a board, and this compared it against a project's internal ID — which never matches, so
/// every reload fell through to whichever board sorted first, and the picker was reset in front of somebody who
/// had chosen. Going through [`project_by_prefix`] also means a board renamed since it was remembered is still
/// found, and one somebody lost access to falls through rather than leaving the screen on nothing.
fn initial_project(
    projects: &[ProjectResponse],
    search: &str,
    remembered: Option<String>,
) -> String {
    if let Some((prefix, _)) = task_id_parts(search.trim()) {
        if let Some(project) = project_by_prefix(projects, prefix) {
            return project.prefix.clone();
        }
    }

    remembered
        .and_then(|prefix| project_by_prefix(projects, &prefix))
        .map(|project| project.prefix.clone())
        .or_else(|| first_to_open(projects))
        .unwrap_or_default()
}

/// The board to open when nothing named one — no handle in the box, nothing remembered, or what was
/// remembered is gone.
///
/// **A live board is preferred, and an archived one is still taken when every board is archived.** Both
/// halves matter. Landing a fresh browser on a project somebody deliberately put away is the one way
/// archiving could fail to hide it; but refusing to pick at all would leave the screen on nothing while the
/// render-time fallback below still drew the first project — the state and the control telling two stories,
/// which is exactly what the `selected`-on-the-option comments warn about. Taking an archived board here is
/// safe because the picker keeps an option for whatever is open.
fn first_to_open(projects: &[ProjectResponse]) -> Option<String> {
    projects
        .iter()
        .find(|itm| !itm.archived)
        .or_else(|| projects.first())
        .map(|itm| itm.prefix.clone())
}

/// One box, two ways of narrowing, decided by the shape of what is in it.
///
/// A handle narrows by its digits as they are typed, on the board its prefix has already switched to.
/// Anything else is text matched against the face of a card.
fn matches_search(task: &TaskResponse, search: &str) -> bool {
    let search = search.trim();

    if search.is_empty() {
        return true;
    }

    match task_id_parts(search) {
        Some((_, number)) => id_number_starts_with(&task.id, number),
        None => matches_text(task, &search.to_lowercase()),
    }
}

/// Whether a task's number begins with the digits typed so far.
///
/// Leading zeros come off both sides first. Ids are no longer padded, so this is now about what gets PASTED
/// into the box: `RMS-000042` out of an old link or an old chat has to find the task that now calls itself
/// `RMS-42`, and comparing as written would say there is no such card.
fn id_number_starts_with(task_id: &str, digits: &str) -> bool {
    let Some((_, number)) = task_id.rsplit_once('-') else {
        return false;
    };

    number
        .trim_start_matches('0')
        .starts_with(digits.trim_start_matches('0'))
}

/// Everything on a card that is worth searching: its id, its text, its labels and who has it.
///
/// Not the comments. A thread can be long and mostly discussion, so matching it would return cards whose
/// visible face has nothing to do with what was typed — which reads as a broken filter rather than a
/// thorough one.
fn matches_text(task: &TaskResponse, needle: &str) -> bool {
    task.id.to_lowercase().contains(needle)
        || task.text.to_lowercase().contains(needle)
        || task
            .labels
            .iter()
            .any(|itm| itm.to_lowercase().contains(needle))
        || task
            .assignee
            .as_ref()
            .is_some_and(|itm| itm.to_lowercase().contains(needle))
        || task
            .assignee_name
            .as_ref()
            .is_some_and(|itm| itm.to_lowercase().contains(needle))
}

/// Everything this screen holds, in one struct behind one signal — the house shape.
///
/// The two `DataState`s are why the board works at all now. They are read in the RENDER body, which is what
/// subscribes the component to them; the previous version read its dependencies inside `use_future`, which
/// spawns once and tracks nothing, so the board loaded exactly never.
#[derive(Default)]
struct ComponentState {
    projects: DataState<Vec<ProjectResponse>>,
    /// Keyed on `selected`: choosing another board resets this, and the next render loads it.
    tasks: DataState<Vec<TaskResponse>>,
    /// Which board is on screen, by PREFIX — the only name a project has on this side. See
    /// `ProjectResponse` in the shared crate for why there is no id to hold instead.
    selected: String,
    search: String,
    /// Empty means "any". Ids rather than indexes, so a board changing under a filter cannot silently move
    /// it to a different type.
    kind_filter: String,
    assignee_filter: String,
    /// Which goal's work to show, by goal handle — or [`NO_GOAL`] for the work that is under none. Empty is
    /// every task, which is what a board is.
    goal_filter: String,
    /// The handle of the card being dragged, if one is. Kept here rather than read back out of the drag's
    /// `dataTransfer`: the payload is set there too, because some browsers will not start a drag without it,
    /// but a signal is what the drop handler can rely on.
    dragging: Option<String>,
    /// The column the pointer is over while dragging, so it can say it will accept the card. Cleared on the
    /// drop and on the drag ending anywhere, including outside the board.
    drop_target: Option<String>,
}

impl ComponentState {
    /// Everything default except the box, which the URL seeds.
    fn with_search(search: String) -> Self {
        Self {
            search,
            ..Default::default()
        }
    }

    /// Switch boards. One method because the two halves are coupled: leaving `tasks` alone would show the
    /// previous project's cards under the new project's name.
    fn select(&mut self, prefix: String) {
        if self.selected == prefix {
            return;
        }

        self.selected = prefix;
        self.tasks.reset();
        // A drag cannot survive the board changing under it — the card it holds is not on this one.
        self.dragging = None;
        self.drop_target = None;
    }

    /// Give up on a drag, wherever it ended.
    fn drag_ended(&mut self) {
        self.dragging = None;
        self.drop_target = None;
    }
}

/// What was typed, plus the board it implies.
///
/// Not a method on `ComponentState`: the projects come from a `DataState` the state does not own, and this
/// reads them. A handle carries the answer to "which board" in its prefix, and following it is the
/// difference between `RMS-42` finding the task and finding nothing because the wrong board was on screen.
fn set_search(cs: &mut ComponentState, search: String, projects: &[ProjectResponse]) {
    cs.search = search;

    if let Some((prefix, _)) = task_id_parts(cs.search.trim()) {
        // Through `project_by_prefix` rather than straight off the handle: what was typed may be a prefix the
        // project has since renamed away from, and the board is named by the one it holds NOW.
        if let Some(project) = project_by_prefix(projects, prefix) {
            let prefix = project.prefix.clone();
            cs.select(prefix);
        }
    }
}

/// The projects this person may see, loading them on first render.
///
/// The socket starts HERE, once the list is in hand — not in the shell on the way past. Until a project is
/// known there is nothing to subscribe to, and the shell was starting it during render, which is a write to
/// a signal in the render body and the one thing `dioxus-design-patterns` §16 says never to do.
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
                        let remembered = crate::web::storage::get_last_project();
                        // The box may already hold a handle — the URL seeds it before anything is loaded —
                        // and then it, not the remembered board, decides where this lands.
                        let search = cs.peek().search.clone();

                        let initial = initial_project(&response.projects, &search, remembered);

                        let mut write = cs.write();
                        write.selected = initial;
                        write.projects.set_loaded(response.projects);
                        drop(write);

                        // Data is in. Now the sockets.
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

/// The selected board's tasks.
///
/// Nothing to load until a project is chosen, which is an empty board rather than a spinner — the picker is
/// already on screen and a spinner there would suggest something is coming.
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

                // `false`: the live board. A push carries the project's whole history, and this screen
                // filters it the same way — see `visible_on_the_board`.
                match crate::api::get_tasks(&project, false).await {
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

fn render_loading() -> Element {
    rsx! {
        div { class: "loading-note", "Loading…" }
    }
}

fn render_error(message: &str) -> Element {
    rsx! {
        div { class: "error-banner", "{message}" }
    }
}

/// The title, the project picker and the filters. Its own component so the loading path and the loaded path
/// draw the same header — otherwise the screen jumps every time a board is switched.
#[component]
fn RenderHeader(
    projects: Vec<ProjectResponse>,
    current: ProjectResponse,
    cs: Signal<ComponentState>,
    assignees: Vec<(String, String)>,
    goals: Vec<(String, String)>,
) -> Element {
    let cs_ra = cs.read();
    let selected_prefix = cs_ra.selected.clone();
    let search_text = cs_ra.search.clone();
    let kind_wanted = cs_ra.kind_filter.clone();
    let assignee_wanted = cs_ra.assignee_filter.clone();
    let goal_wanted = cs_ra.goal_filter.clone();
    drop(cs_ra);

    let mut cs = cs;

    // The handler owns its copy: the list below is borrowed by the rsx loop, and an event handler outlives
    // the render that built it.
    let projects_for_search = projects.clone();

    rsx! {
        div { class: "page-header",
            h1 { class: "page-title", "Home" }
            div { class: "project-picker",
                // `selected` on the matching option, not `value` on the select: HTML decides a dropdown's
                // shown item from the option's attribute, so the remembered project was restored into the
                // state but the control still displayed the first entry.
                select {
                    onchange: move |event| cs.write().select(event.value()),
                    // Valued by PREFIX, like every other project-shaped control in this client and like the
                    // api underneath it — there is no id on this side to carry instead.
                    //
                    // Archived boards are left out, which is the whole of what archiving does — except for
                    // the one currently open, which stays. That exception is not politeness: by the comment
                    // above, the control shows whichever option carries `selected`, so an open board with no
                    // option would leave the dropdown displaying the FIRST project while the state held a
                    // different one. A board reached by link stays visible in the control that named it.
                    for project in projects.iter().filter(|itm| !itm.archived || itm.prefix == selected_prefix) {
                        option {
                            value: "{project.prefix}",
                            selected: project.prefix == selected_prefix,
                            "{project.prefix} · {project.name}"
                        }
                    }
                }
                select {
                    onchange: move |event| cs.write().kind_filter = event.value(),
                    option { value: "", selected: kind_wanted.is_empty(), "Any type" }
                    for kind in current.kinds.iter() {
                        option {
                            value: "{kind.id}",
                            selected: kind.id == kind_wanted,
                            "{kind.name}"
                        }
                    }
                }
                select {
                    onchange: move |event| cs.write().assignee_filter = event.value(),
                    option { value: "", selected: assignee_wanted.is_empty(), "Anyone" }
                    option {
                        value: "{UNASSIGNED}",
                        selected: assignee_wanted == UNASSIGNED,
                        "Unassigned"
                    }
                    for who in assignees.iter() {
                        option {
                            value: "{who.0}",
                            selected: who.0 == assignee_wanted,
                            "{who.1}"
                        }
                    }
                }
                // Last of the four, after the assignee. The three before it are one word each and hold their
                // width; a goal is a sentence somebody wrote, so this is the box that grows — and a box that
                // grows must not be able to shove the ones after it around.
                select {
                    onchange: move |event| cs.write().goal_filter = event.value(),
                    option { value: "", selected: goal_wanted.is_empty(), "Any goal" }
                    option {
                        value: "{NO_GOAL}",
                        selected: goal_wanted == NO_GOAL,
                        "No goal"
                    }
                    for goal in goals.iter() {
                        option {
                            value: "{goal.0}",
                            selected: goal.0 == goal_wanted,
                            "{goal.1}"
                        }
                    }
                }
            }
            // Held at the right edge. The selects on the left say which board and which slice of it; this
            // says what is being looked for, which is a different question.
            div { class: "board-search-box",
                input {
                    class: "board-search",
                    r#type: "text",
                    placeholder: "Search, or a task id — RMS-42",
                    value: "{search_text}",
                    oninput: move |event| {
                        let value = event.value();
                        set_search(&mut cs.write(), value.clone(), &projects_for_search);
                        // `replace`, not `push`: a keystroke is not somewhere the Back button should have to
                        // walk through. The URL is a projection of the box — the box was already set above.
                        navigator()
                            .replace(crate::AppRoute::Home {
                                search: url_search(&value),
                            });
                    },
                    onkeydown: move |event| {
                        if event.key() == Key::Enter {
                            let query = cs.peek().search.trim().to_string();

                            if looks_like_a_task_id(&query) {
                                spawn(async move {
                                    if let Ok(found) = crate::api::find_task(&query).await {
                                        crate::dialogs::open(
                                            crate::dialogs::DialogState::ViewTask { found },
                                        );
                                    }
                                });
                            }
                        }
                    },
                }
                // Only when there is something to clear. A cross over an empty box is a button that does
                // nothing, sitting where the eye looks first.
                if !search_text.is_empty() {
                    button {
                        class: "board-search-clear",
                        r#type: "button",
                        title: "Clear the search",
                        onclick: move |_| {
                            // Written straight rather than through `set_search`: that one exists to follow a
                            // handle to its board, and an empty box names no handle and no board. The board
                            // stays where it is — clearing the search is not leaving the project.
                            cs.write().search = String::new();
                            navigator().replace(crate::AppRoute::Home { search: None });
                        },
                        "×"
                    }
                }
            }
        }
    }
}

/// What the box holds, as the URL should carry it: nothing at all when there is nothing to search for.
///
/// `None` rather than an empty string so a board nobody is searching stays `/?` instead of `/?search=`.
fn url_search(value: &str) -> Option<String> {
    let value = value.trim();

    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// One column as the board draws it, anchors included.
#[derive(Clone, PartialEq)]
pub struct BoardColumn {
    pub id: String,
    pub name: String,
    pub description: String,
}

/// The project's configured columns with the two anchors put back at either end.
///
/// The wire model leaves `todo` and `done` out — they exist in every project by definition — so every
/// reader adds them, and this is where this one does it.
fn board_columns(project: &ProjectResponse) -> Vec<BoardColumn> {
    let mut result = Vec::with_capacity(project.columns.len() + 2);

    result.push(BoardColumn {
        id: COLUMN_ID_TODO.to_string(),
        name: "Todo".to_string(),
        description: String::new(),
    });

    let mut middle: Vec<&task_manager_shared::projects::ProjectColumnResponse> =
        project.columns.iter().collect();
    middle.sort_by_key(|itm| itm.order);

    for column in middle {
        result.push(BoardColumn {
            id: column.id.clone(),
            name: column.name.clone(),
            description: column.description.clone(),
        });
    }

    result.push(BoardColumn {
        id: COLUMN_ID_DONE.to_string(),
        name: "Done".to_string(),
        description: String::new(),
    });

    result
}

#[component]
fn RenderColumn(
    column: BoardColumn,
    project: ProjectResponse,
    tasks: Vec<TaskResponse>,
    cs: Signal<ComponentState>,
) -> Element {
    let mut cs = cs;
    // The server has already folded an unknown status into `todo`, so a plain comparison is enough here —
    // the leniency lives in one place rather than being re-implemented per client.
    let mut in_column: Vec<&TaskResponse> =
        tasks.iter().filter(|itm| itm.status == column.id).collect();

    board_order(&mut in_column);

    let cs_ra = cs.read();
    let dragging = cs_ra.dragging.clone();
    let hovering = cs_ra.drop_target.as_deref() == Some(column.id.as_str());
    drop(cs_ra);

    // Highlighted only while a card is actually in flight AND over this column: a column that lights up on
    // an ordinary mouse-over would be promising something it cannot do.
    let class = if dragging.is_some() && hovering {
        "board-column drop-target"
    } else {
        "board-column"
    };

    let column_id = column.id.clone();
    let column_name = column.name.clone();
    let over_id = column.id.clone();
    let done = column.id == COLUMN_ID_DONE;

    rsx! {
        div {
            class: "{class}",
            // `prevent_default` is what makes a drop possible at all: without it the browser treats every
            // element as refusing the drag, and `ondrop` never fires. That is the single most common way a
            // drag-and-drop is built and then quietly does nothing.
            ondragover: move |event| {
                event.prevent_default();

                let mut write = cs.write();
                if write.dragging.is_some() && write.drop_target.as_deref() != Some(over_id.as_str()) {
                    write.drop_target = Some(over_id.clone());
                }
            },
            ondrop: move |event| {
                event.prevent_default();

                let Some(handle) = cs.write().dragging.take() else {
                    return;
                };

                cs.write().drop_target = None;

                let status = column_id.clone();
                let column_name = column_name.clone();

                // Landing owes an explanation, and the server refuses the move without one — so the drop
                // asks for it rather than failing. Every other column moves straight away: the push comes
                // back with the whole board, so there is nothing to write back here.
                if done {
                    crate::dialogs::open(crate::dialogs::DialogState::LandTask {
                        handle,
                        status,
                    });
                    return;
                }

                spawn(async move {
                    if let Err(err) = crate::api::move_task(&handle, &status, None).await {
                        crate::dialogs::open(crate::dialogs::DialogState::Message {
                            title: format!("{handle} did not move to {column_name}"),
                            text: err.message,
                        });
                    }
                });
            },

            div { class: "board-column-header",
                span { class: "board-column-name", "{column.name}" }
                span { class: "board-column-count", "{in_column.len()}" }
            }
            if !column.description.trim().is_empty() {
                div { class: "board-column-description", "{column.description}" }
            }
            // The only part that scrolls. The header and the description stay in place, so which column
            // you are looking at is still answerable once the cards have moved.
            div { class: "board-column-body",
                for task in in_column {
                    RenderSticker {
                        key: "{task.id}",
                        task: task.clone(),
                        project: project.clone(),
                        cs,
                    }
                }
            }
        }
    }
}

/// One card, and deliberately almost nothing: its handle and its title.
///
/// The whole task — the text, the thread, who has it, what it waits on — is behind a double-click on the
/// card, and only there. A column of full cards is a wall of Markdown you have to read to scan, which is the
/// opposite of what a board is for; a column of titles is a list you can take in at a glance.
///
/// The one button on a card copies a link to the task, which is the thing the card cannot do for itself: the
/// task is a double-click away for whoever is looking at the board, and a paste away for whoever is not.
#[component]
fn RenderSticker(
    task: TaskResponse,
    project: ProjectResponse,
    cs: Signal<ComponentState>,
) -> Element {
    let mut cs = cs;
    let kind = task
        .kind
        .as_ref()
        .and_then(|kind_id| project.kinds.iter().find(|itm| &itm.id == kind_id));

    let kind_color = kind
        .map(|itm| KindColor::parse_or_default(&itm.color))
        .unwrap_or_default();

    // The type is on the card twice, on purpose: as its label beside the handle, which is what you read, and
    // as the colour of the left edge, which is what you see without reading — a column of edges tells you how
    // the work is made up before you have looked at a single card.
    let border = kind
        .map(|_| format!("border-left-color: {}", kind_color.hex()))
        .unwrap_or_default();

    let title = task_manager_shared::task_title::task_title(&task.text);

    // An unrecognised value reads as Normal, which is also what a card written before priorities existed
    // carries — so neither draws a badge, and neither jumps the queue.
    let priority = Priority::parse_or_default(&task.priority);

    let assignee = task
        .assignee_name
        .clone()
        .or_else(|| task.assignee.clone())
        .unwrap_or_else(|| "Unassigned".to_string());

    // Unknown colour falls back to the default swatch rather than to nothing, so a goal coloured by a build
    // that knew one more swatch still draws a band.
    let goal_hex = KindColor::parse_or_default(task.goal_color.as_deref().unwrap_or_default()).hex();

    let goal_title = task
        .goal_name
        .clone()
        .unwrap_or_else(|| "Goal".to_string());

    // Built here rather than fetched: this side already holds the whole task and the project it is on, so
    // the card opens instantly and without a round trip. `archived` is false by definition — a card that is
    // drawn is on the board.
    let found_on_the_card = crate::api::find_task_locally(&task, &project);

    // Composed at render, not on the click: the handler is `move` and cannot borrow the task, and a link is
    // a `format!` of two strings — cheaper to build than to reason about.
    let link = crate::web::task_link(&task.id);
    let task_for_message = task.id.clone();

    // Per card, because "copied" is a fact about the button that was clicked, not about the board. It is
    // render state and lives nowhere else — a receipt has nothing to say to the server or to the next visit.
    let mut copied = use_signal(|| false);
    let just_copied = *copied.read();

    let dragged = task.id.clone();
    let being_dragged = cs.read().dragging.as_deref() == Some(task.id.as_str());

    rsx! {
        div {
            // Deleted first: it is the strongest thing to say about a card, and one only ever appears here
            // because somebody searched for it.
            class: if task.deleted_unix_seconds.is_some() {
                "sticker deleted"
            } else if being_dragged {
                "sticker dragging"
            } else if task.blocked {
                "sticker blocked"
            } else {
                "sticker"
            },
            style: "{border}",
            // The card is the drag handle — all of it, so there is nothing to aim at.
            draggable: true,
            ondragstart: move |event| {
                // Written into the drag's own payload AND into the state. The payload is what some browsers
                // insist on before they will start a drag at all; the state is what the drop reads, because
                // it is the one of the two that cannot be emptied by the browser on the way.
                let _ = event.data_transfer().set_data("text/plain", &dragged);
                cs.write().dragging = Some(dragged.clone());
            },
            // Fires wherever the drag ended, including nowhere — without it, a card abandoned outside the
            // board would stay dimmed and every column would keep offering to accept it.
            ondragend: move |_| cs.write().drag_ended(),
            // The whole card, and now the only way in — the eye that used to open it is a copy-link button.
            // Double rather than single, because a single click on a card is how you select one and this
            // board has no selection: a stray click must not throw a dialog in front of somebody who was
            // only scrolling.
            ondoubleclick: move |_| {
                crate::dialogs::open(crate::dialogs::DialogState::ViewTask {
                    found: found_on_the_card.clone(),
                });
            },

            // The card's FIRST child and full width, which is what makes it read as a header of the card
            // rather than as a chip inside it: a column of coloured bands says how the work divides between
            // epics before a single title has been read. Id and title both — the title is what a person
            // reads, the id is what they type back into the search box or to an agent.
            //
            // No negative margins. Reaching the card's edges used to be arithmetic against the card's own
            // padding and its 3px left border, and it came out hanging over the right edge; the padding now
            // lives on `.sticker-body` and there is nothing to calculate.
            if let Some(goal) = task.goal.as_ref() {
                div {
                    class: "sticker-goal",
                    style: "background: {goal_hex}",
                    // The band is one line wide and the name is cut off with an ellipsis when it does not
                    // fit, which on a narrow column is most of the time — so hovering the band says the
                    // whole thing. The handle is in the tooltip too: it is what somebody types back, and a
                    // reader who is hovering to find out which epic this is wants both halves of the answer.
                    title: "{goal} · {goal_title}",
                    span { class: "sticker-goal-id", "{goal}" }
                    span { class: "sticker-goal-name", "{goal_title}" }
                }
            }

            div { class: "sticker-body",
                div { class: "sticker-top",
                    span { class: "sticker-id", "{task.id}" }
                    if let Some(kind) = kind {
                        span {
                            class: "sticker-kind",
                            style: "background: {kind_color.hex()}",
                            title: "{kind.description}",
                            // Only when this build actually has the file: a name stored before an icon was
                            // renamed away draws as no icon rather than as a broken image.
                            if crate::web::icon_exists(&kind.icon) {
                                img {
                                    class: "sticker-kind-icon",
                                    src: "{crate::web::icon_url(&kind.icon)}",
                                    alt: "",
                                }
                            }
                            "{kind.name}"
                        }
                    }
                    // Copying the link, not opening the task: a double-click on the card does the opening,
                    // and it always did — the eye that used to sit here was a second way to do the one
                    // thing the card already does. Handing a task to somebody else had no way at all.
                    button {
                        class: if just_copied { "sticker-link copied" } else { "sticker-link" },
                        title: "Copy a link to {task.id}",
                        onclick: move |event| {
                            // The card is the drag handle and the card opens the task; neither should
                            // happen because somebody reached for this button.
                            event.stop_propagation();

                            if crate::web::copy_to_clipboard(&link) {
                                copied.set(true);

                                // Back to the chain link on its own. The tick is the only answer there is
                                // — nothing else on screen changes when a link goes to the clipboard — and
                                // one that stayed would read as a state of the task rather than as a
                                // receipt for a click.
                                spawn(async move {
                                    dioxus_utils::js::sleep(std::time::Duration::from_millis(1_200))
                                        .await;
                                    copied.set(false);
                                });
                            } else {
                                // No clipboard means the page is not in a secure context. Showing the link
                                // is not a consolation prize: it is still copyable, by hand, which is what
                                // the reader wanted.
                                crate::dialogs::open(crate::dialogs::DialogState::Message {
                                    title: format!("Link to {task_for_message}"),
                                    text: link.clone(),
                                });
                            }
                        },
                        // A second click on a copy button is somebody making sure, and without this it
                        // would reach the card as a double-click and throw the task dialog at them.
                        ondoubleclick: move |event| event.stop_propagation(),
                        if just_copied { "✓" } else { "🔗" }
                    }
                }

                div { class: "sticker-title", "{title}" }

                // Under the title, because the title is what the card is FOR and who has it is the next
                // question — and it is the same question on every card, so it belongs in the same place on
                // every card.
                //
                // Who has it on the left, how it is ranked on the far right, the counters between. Everything
                // here is a fact ABOUT the task rather than part of it, which is why they share a row, and
                // the two ends are the two questions asked most: who, and how urgent.
                //
                // The counters say only HOW MANY. What they count is in the dialog, and a card that listed
                // the ids it waits on was one of the things that made a column unreadable.
                div { class: "sticker-bottom",
                    span {
                        class: if task.assignee.is_some() { "sticker-assignee" } else { "sticker-assignee unassigned" },
                        "{assignee}"
                    }
                    div { class: "sticker-counters",
                        if !task.comments.is_empty() {
                            span { title: "{task.comments.len()} comments", "💬 {task.comments.len()}" }
                        }
                        if !task.depends_on.is_empty() {
                            span {
                                title: "Waiting on {task.depends_on.len()} task(s): {handles(&task.depends_on)}",
                                "⬇ {task.depends_on.len()}"
                            }
                        }
                        if !task.blocks.is_empty() {
                            span {
                                title: "{task.blocks.len()} task(s) waiting on this one: {handles(&task.blocks)}",
                                "⬆ {task.blocks.len()}"
                            }
                        }
                    }
                    // Last, so it sits against the card's right edge whether or not there are counters to
                    // its left — a badge that moved with the number of comments would be a badge you have
                    // to look for.
                    //
                    // Only when somebody actually ranked it. Normal is what most work is, so a badge on
                    // every card would be a badge nobody reads, and the position in the column already says
                    // as much: this is here to answer "why is THAT at the top". It used to be in the top
                    // row, where it fought the type chip and the handle for a line that has no room for
                    // three things.
                    if priority.is_worth_showing() {
                        span {
                            class: "sticker-priority",
                            style: "background: {priority.hex()}",
                            title: "Priority: {priority.title()}",
                            "{priority.title()}"
                        }
                    }
                }
            }
        }
    }
}

/// Order a column: **the most urgent card first**, and within one priority the work that belongs to a goal
/// first, grouped by goal.
///
/// Priority leads, and it has to: the whole of what a priority means on this product is where the card sits,
/// so a grouping that outranked it would make Super High mean "near the top of its own group", which is not
/// a priority at all. Two cards ranked the same are then arranged as they always were — cards under a goal
/// above the loose ones, so a column reads as "this is what the epics need" and then "and this is loose", and
/// cards of one goal adjacent, without which a column of coloured bands is stripes rather than groups.
///
/// A STABLE sort, so inside a group the board's own order — oldest first, as the server sent it — survives.
/// Which goal leads is whatever the handles compare to; arbitrary, but the same on every repaint, and a group
/// that moved each time the board pushed would be worse than an arbitrary order.
fn board_order(tasks: &mut [&TaskResponse]) {
    tasks.sort_by(|left, right| {
        Priority::order_of(&left.priority)
            .cmp(&Priority::order_of(&right.priority))
            .then_with(|| left.goal.is_none().cmp(&right.goal.is_none()))
            .then_with(|| left.goal.cmp(&right.goal))
    });
}

/// A list of handles as a tooltip reads them.
fn handles(ids: &[String]) -> String {
    ids.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn task(id: &str, text: &str, labels: &[&str], assignee: Option<&str>) -> TaskResponse {
        TaskResponse {
            id: id.to_string(),
            project: "P".to_string(),
            text: text.to_string(),
            status: COLUMN_ID_TODO.to_string(),
            // Blank rather than `normal`, so every ordering test also proves that a task written before
            // priorities existed sorts where a Normal one does.
            priority: String::new(),
            kind: None,
            goal: None,
            goal_name: None,
            goal_color: None,
            assignee: assignee.map(|itm| itm.to_string()),
            assignee_name: None,
            labels: labels.iter().map(|itm| itm.to_string()).collect(),
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
            closed_unix_seconds: None,
            deleted_unix_seconds: None,
        }
    }

    /// The board draws the live window out of a list that now carries the whole history — the push does not
    /// filter any more, so this screen must. Getting it wrong in either direction is quiet: too strict and
    /// work vanishes off the board, too lax and Done grows without bound.
    #[test]
    fn the_board_hides_work_past_the_projects_archive_window() {
        let now = 1_000 * 24 * 60 * 60;

        let mut fresh = task("RMS-1", "just closed", &[], None);
        fresh.status = COLUMN_ID_DONE.to_string();
        fresh.closed_unix_seconds = Some(now - 60);

        let mut old_news = task("RMS-2", "closed ages ago", &[], None);
        old_news.status = COLUMN_ID_DONE.to_string();
        old_news.closed_unix_seconds = Some(now - 30 * 24 * 60 * 60);

        let open = task("RMS-3", "still open", &[], None);

        let archived = |task: &TaskResponse, days| {
            task_manager_shared::tasks::is_task_archived(task, days, now)
        };

        assert!(!archived(&fresh, None));
        assert!(archived(&old_news, None));
        assert!(!archived(&open, None), "open work never archives, however old");

        // And the window is the project's: two days hides what the default would still be drawing.
        assert!(archived(&fresh, Some(2)) == false, "a minute is inside two days");
        assert!(archived(&old_news, Some(60)) == false, "thirty days is inside sixty");
    }

    /// Which of the two things the box does is decided here, so the shapes are worth pinning.
    #[test]
    fn a_handle_is_told_apart_from_a_search() {
        assert!(looks_like_a_task_id("RMS-42"));
        assert!(
            looks_like_a_task_id("RMS-000042"),
            "the padded spelling is not composed any more, but it is still an id somebody may paste"
        );
        assert!(
            looks_like_a_task_id("TM_2-7"),
            "an underscore is legal in a prefix"
        );

        // Words that happen to contain a dash must NOT be taken for an id, or searching for one of our own
        // type names would fire a lookup instead of filtering.
        assert!(!looks_like_a_task_id("tech-debt"));
        assert!(!looks_like_a_task_id("in-progress-review"));
        assert!(!looks_like_a_task_id("RMS-"));
        assert!(!looks_like_a_task_id("-42"));
        assert!(!looks_like_a_task_id("42"));
        assert!(!looks_like_a_task_id(""));
        assert!(!looks_like_a_task_id("fix the login bug"));
    }

    /// A column id with a dash in it is a plausible thing to type, and it is not an id.
    #[test]
    fn a_prefix_may_not_contain_a_dash() {
        assert!(!looks_like_a_task_id("in-progress-2"));
    }

    fn project(prefix: &str, history: &[&str]) -> ProjectResponse {
        ProjectResponse {
            name: prefix.to_string(),
            description: String::new(),
            prefix: prefix.to_string(),
            prefix_history: history.iter().map(|itm| itm.to_string()).collect(),
            columns: Vec::new(),
            column_template_id: None,
            column_template_name: None,
            kinds: Vec::new(),
            kind_template_id: None,
            kind_template_name: None,
            members: Vec::new(),
            tasks_amount: 0,
            archive_days: None,
            archived: false,
        }
    }

    /// The order is the point: a prefix a project holds NOW beats one it used to hold, because a freed
    /// prefix can be taken by somebody else and history would then send the reader to the wrong board.
    #[test]
    fn the_present_prefix_outranks_a_historical_one() {
        // TM used to be RMS; RMS is somebody else now. Which is exactly the trap: one project's history
        // names another project's present.
        let projects = vec![project("TM", &["RMS"]), project("RMS", &["PROTO"])];

        assert_eq!(
            project_by_prefix(&projects, "RMS").map(|itm| itm.prefix.as_str()),
            Some("RMS"),
            "the project holding RMS today, not the one that used to"
        );

        assert_eq!(
            project_by_prefix(&projects, "PROTO").map(|itm| itm.prefix.as_str()),
            Some("RMS"),
            "history answers when nobody holds it now"
        );

        assert_eq!(
            project_by_prefix(&projects, "rms").map(|itm| itm.prefix.as_str()),
            Some("RMS"),
            "a prefix is typed in whatever case comes to hand"
        );

        assert!(project_by_prefix(&projects, "NOPE").is_none());
    }

    /// Nothing named a board, so one gets picked — and archiving is what decides which.
    #[test]
    fn the_board_picked_by_default_is_a_live_one() {
        let mut put_away = project("AAA", &[]);
        put_away.archived = true;

        let projects = vec![put_away, project("BBB", &[])];

        assert_eq!(
            initial_project(&projects, "", None),
            "BBB",
            "an archived board sorting first must not become the one a fresh browser opens on"
        );

        // A remembered board still wins outright, archived or not: somebody chose it, and a board reached
        // deliberately is exactly the case archiving is not meant to interfere with.
        assert_eq!(
            initial_project(&projects, "", Some("AAA".to_string())),
            "AAA"
        );

        // And so does a handle in the box, which is what a link carrying one is for.
        assert_eq!(initial_project(&projects, "AAA-42", None), "AAA");
    }

    /// The other half of the rule, and the reason it is not a plain filter: with every board archived there
    /// is still a board to open, or the screen would sit on nothing while the picker drew a project.
    #[test]
    fn a_board_is_still_picked_when_every_one_is_archived() {
        let mut first = project("AAA", &[]);
        first.archived = true;
        let mut second = project("BBB", &[]);
        second.archived = true;

        assert_eq!(initial_project(&[first, second], "", None), "AAA");
        assert_eq!(initial_project(&[], "", None), "");
    }

    /// Which board a page load lands on. A handle in the URL beats the remembered board — the handle is what
    /// the link was sent for.
    /// Which board a page load lands on, and in what order the three answers are consulted. Everything here
    /// is a PREFIX — what the picker holds, what storage keeps, what the api is called with — which is the
    /// whole point: the reload bug this test was written for was a remembered prefix compared against a
    /// project's internal id, a comparison that can only fail, so every reload fell through to whichever
    /// board sorted first.
    #[test]
    fn a_handle_in_the_url_decides_the_board() {
        let projects = vec![project("AAA", &[]), project("BBB", &["OLD"])];

        assert_eq!(
            initial_project(&projects, "BBB-42", Some("AAA".to_string())),
            "BBB",
            "a handle in the url beats the remembered board — the handle is what the link was sent for"
        );

        assert_eq!(
            initial_project(&projects, "OLD-42", Some("AAA".to_string())),
            "BBB",
            "a renamed prefix still names its board"
        );

        assert_eq!(
            initial_project(&projects, "", Some("BBB".to_string())),
            "BBB",
            "nothing typed: the remembered board"
        );

        assert_eq!(
            initial_project(&projects, "", Some("bbb".to_string())),
            "BBB",
            "and the case it was typed in is not what decides it"
        );

        assert_eq!(
            initial_project(&projects, "", Some("OLD".to_string())),
            "BBB",
            "a board renamed since it was remembered is still the board it was"
        );

        assert_eq!(
            initial_project(&projects, "login bug", Some("BBB".to_string())),
            "BBB",
            "a plain search says nothing about which board"
        );

        assert_eq!(
            initial_project(&projects, "ZZZ-1", Some("BBB".to_string())),
            "BBB",
            "a prefix nobody holds falls through rather than emptying the screen"
        );

        assert_eq!(
            initial_project(&projects, "", Some("GONE".to_string())),
            "AAA",
            "a board somebody lost access to falls through to the first they can see"
        );

        assert_eq!(initial_project(&projects, "", None), "AAA");
        assert_eq!(initial_project(&[], "AAA-1", None), "");
    }

    /// Typing a handle narrows the board digit by digit — and a padded handle pasted out of an old link has
    /// to land on the same card, which is the case that needs saying now that ids read `RMS-42`.
    #[test]
    fn a_handle_narrows_by_number_as_it_is_typed() {
        let one = task("RMS-42", "Fix the login redirect", &[], None);
        let two = task("RMS-4", "Something else", &[], None);
        let three = task("RMS-420", "And another", &[], None);

        assert!(matches_search(&one, "RMS-4"));
        assert!(matches_search(&two, "RMS-4"));
        assert!(matches_search(&three, "RMS-4"));

        assert!(matches_search(&one, "RMS-42"));
        assert!(!matches_search(&two, "RMS-42"));
        assert!(matches_search(&three, "RMS-42"));

        assert!(
            matches_search(&one, "RMS-000042"),
            "the padded form as well, so an old link still finds its card"
        );
        assert!(!matches_search(&one, "RMS-43"));

        // The prefix chooses the board, not the cards on it: an id looked up by a prefix the project has
        // since renamed away from must still find its task, whose id now carries the new prefix.
        assert!(matches_search(&one, "OLD-42"));

        assert!(matches_search(&one, ""), "an empty box narrows nothing");
    }

    /// Text and handles are the same box, and the two must not bleed into each other.
    #[test]
    fn text_is_still_matched_as_text() {
        let one = task("RMS-42", "Fix the login redirect", &["auth"], None);

        assert!(matches_search(&one, "login"));
        assert!(matches_search(&one, "AUTH"));
        assert!(!matches_search(&one, "logout"));
    }

    #[test]
    fn the_filter_looks_at_the_face_of_a_card() {
        let one = task(
            "RMS-1",
            "Fix the login redirect",
            &["auth"],
            Some("ann@x.io"),
        );

        assert!(matches_text(&one, "login"));
        assert!(matches_text(&one, "rms-1"), "the id is searchable too");
        assert!(matches_text(&one, "auth"), "and its labels");
        assert!(matches_text(&one, "ann"), "and who has it");
        assert!(!matches_text(&one, "logout"));
    }

    /// Case must not matter: what gets typed is lower case and what is stored is however it was written.
    #[test]
    fn the_type_filter_is_exact_and_empty_means_any() {
        let mut one = task("RMS-1", "text", &[], None);
        one.kind = Some("bug".to_string());

        assert!(matches_kind(&one, ""), "empty means any");
        assert!(matches_kind(&one, "bug"));
        assert!(!matches_kind(&one, "feature"));

        let none = task("RMS-2", "text", &[], None);
        assert!(matches_kind(&none, ""));
        assert!(
            !matches_kind(&none, "bug"),
            "a task with no type matches no type"
        );
    }

    /// "Unassigned" needs its own value: empty already means "anyone", and the pile nobody picked up is
    /// exactly what you want to filter for.
    #[test]
    fn the_assignee_filter_tells_anyone_from_unassigned() {
        let taken = task("RMS-1", "text", &[], Some("ann@x.io"));
        let free = task("RMS-2", "text", &[], None);

        assert!(matches_assignee(&taken, ""));
        assert!(matches_assignee(&free, ""));

        assert!(matches_assignee(&taken, "ann@x.io"));
        assert!(!matches_assignee(&free, "ann@x.io"));

        assert!(matches_assignee(&free, UNASSIGNED));
        assert!(!matches_assignee(&taken, UNASSIGNED));
    }

    /// The reserved assignee is stored as `AI` and an address lower-cased, so the compare cannot be exact.
    #[test]
    fn the_assignee_filter_ignores_case() {
        let ai = task("RMS-1", "text", &[], Some("AI"));

        assert!(matches_assignee(&ai, "AI"));
        assert!(matches_assignee(&ai, "ai"));
    }

    #[test]
    fn the_assignee_list_comes_from_the_board_and_is_deduplicated() {
        let mut named = task("RMS-1", "text", &[], Some("ann@x.io"));
        named.assignee_name = Some("Ann".to_string());

        let tasks = vec![
            named,
            task("RMS-2", "text", &[], Some("ANN@x.io")),
            task("RMS-3", "text", &[], Some("AI")),
            task("RMS-4", "text", &[], None),
        ];

        let list = assignees_on_board(&tasks);

        assert_eq!(
            list,
            vec![
                ("AI".to_string(), "AI".to_string()),
                ("ann@x.io".to_string(), "Ann".to_string()),
            ],
            "one entry per person, labelled by name where there is one, and nobody for an unassigned task"
        );
    }

    #[test]
    fn the_filter_ignores_case() {
        let one = task("RMS-1", "Fix the LOGIN redirect", &["Auth"], None);

        assert!(matches_text(&one, "login"));
        assert!(matches_text(&one, "auth"));
    }

    /// Goal-bearing cards rise, same-goal cards group, and the board's own order survives inside a group.
    #[test]
    fn a_column_puts_goals_first_and_keeps_them_together() {
        let mut loose_one = task("RMS-1", "loose", &[], None);
        let mut under_a = task("RMS-2", "a", &[], None);
        let mut loose_two = task("RMS-3", "loose", &[], None);
        let mut under_b = task("RMS-4", "b", &[], None);
        let mut under_a_again = task("RMS-5", "a again", &[], None);

        under_a.goal = Some("RMS-G10".to_string());
        under_b.goal = Some("RMS-G20".to_string());
        under_a_again.goal = Some("RMS-G10".to_string());
        loose_one.goal = None;
        loose_two.goal = None;

        let mut column = vec![
            &loose_one,
            &under_a,
            &loose_two,
            &under_b,
            &under_a_again,
        ];

        board_order(&mut column);

        let ids: Vec<&str> = column.iter().map(|itm| itm.id.as_str()).collect();

        assert_eq!(
            ids,
            vec![
                "RMS-2",
                "RMS-5",
                "RMS-4",
                "RMS-1",
                "RMS-3",
            ],
            "both cards of RMS-G10 first and in board order, then RMS-G20, then the loose pile in board order"
        );
    }

    /// Priority outranks the goal grouping, and that is the whole meaning of the field: a Super High card is
    /// at the top of its COLUMN, not at the top of its own little group. The loose Super High card here beats
    /// two cards that belong to goals.
    #[test]
    fn priority_outranks_the_goal_grouping() {
        let mut normal_under_a = task("RMS-1", "a", &[], None);
        let mut urgent_loose = task("RMS-2", "urgent", &[], None);
        let mut quiet_under_a = task("RMS-3", "later", &[], None);

        normal_under_a.goal = Some("RMS-G10".to_string());
        quiet_under_a.goal = Some("RMS-G10".to_string());

        urgent_loose.priority = "super-high".to_string();
        quiet_under_a.priority = "super-low".to_string();

        let mut column = vec![&normal_under_a, &urgent_loose, &quiet_under_a];

        board_order(&mut column);

        let ids: Vec<&str> = column.iter().map(|itm| itm.id.as_str()).collect();

        assert_eq!(
            ids,
            vec!["RMS-2", "RMS-1", "RMS-3"],
            "super-high first even with no goal, then the normal one, then super-low last"
        );
    }

    /// A card written before priorities existed carries an empty string, and it has to sit exactly where a
    /// `normal` one does — otherwise the first push after a deploy would reshuffle half a board.
    #[test]
    fn an_empty_priority_sits_where_normal_sits() {
        let mut blank = task("RMS-1", "blank", &[], None);
        let mut normal = task("RMS-2", "normal", &[], None);
        let mut low = task("RMS-3", "low", &[], None);

        blank.priority = String::new();
        normal.priority = "normal".to_string();
        low.priority = "low".to_string();

        let mut column = vec![&low, &blank, &normal];

        board_order(&mut column);

        let ids: Vec<&str> = column.iter().map(|itm| itm.id.as_str()).collect();

        assert_eq!(ids, vec!["RMS-1", "RMS-2", "RMS-3"]);
    }
}

#[cfg(test)]
mod deletion_and_goal_filter_tests {
    use super::*;

    fn with_goal(id: &str, goal: Option<&str>) -> TaskResponse {
        let mut result = tests::task(id, "text", &[], None);
        result.goal = goal.map(|itm| itm.to_string());
        result
    }

    /// An empty filter is the board; a handle is one epic; and the reserved value is the work that belongs to
    /// no epic at all — which is a question worth asking, and the reason it needs a value of its own.
    #[test]
    fn the_goal_filter_narrows_to_one_epic_or_to_none() {
        let under = with_goal("RMS-1", Some("RMS-G7"));
        let elsewhere = with_goal("RMS-2", Some("RMS-G8"));
        let loose = with_goal("RMS-3", None);

        for task in [&under, &elsewhere, &loose] {
            assert!(matches_goal(task, ""), "an empty filter is the whole board");
        }

        assert!(matches_goal(&under, "RMS-G7"));
        assert!(!matches_goal(&elsewhere, "RMS-G7"));
        assert!(!matches_goal(&loose, "RMS-G7"));

        assert!(matches_goal(&loose, NO_GOAL));
        assert!(!matches_goal(&under, NO_GOAL));
    }

    /// The options come off the board, so a goal nobody has work under is not offered — an option that
    /// matches nothing is a dead end.
    #[test]
    fn the_goal_options_come_from_the_work_that_is_there() {
        let mut named = with_goal("RMS-1", Some("RMS-G7"));
        named.goal_name = Some("Crypto payments".to_string());

        let same_goal_again = with_goal("RMS-2", Some("RMS-G7"));
        let loose = with_goal("RMS-3", None);

        let goals = goals_on_board(&[named, same_goal_again, loose]);

        assert_eq!(goals.len(), 1, "one entry per goal, not per task");
        assert_eq!(goals[0].0, "RMS-G7");
        assert_eq!(
            goals[0].1, "RMS-G7 · Crypto payments",
            "the handle is what you type back, the name is what you recognise"
        );
    }
}
