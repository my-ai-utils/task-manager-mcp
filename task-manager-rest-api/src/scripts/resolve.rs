use std::sync::Arc;

use crate::board::{
    BoardInner, GoalModel, ProjectModel, ReleaseModel, TaskModel, parse_goal_handle,
    parse_release_handle, parse_task_handle,
};

/// A task and the project it belongs to, resolved together.
///
/// Named rather than a tuple because almost every caller needs both halves and would otherwise have
/// to remember which came first.
pub struct ResolvedTask {
    pub project: Arc<ProjectModel>,
    pub task: Arc<TaskModel>,
}

/// A goal and the project it belongs to. Same shape and same reason as [`ResolvedTask`].
pub struct ResolvedGoal {
    pub project: Arc<ProjectModel>,
    pub goal: Arc<GoalModel>,
}

/// A release and the project it belongs to. Same shape and same reason as [`ResolvedTask`].
pub struct ResolvedRelease {
    pub project: Arc<ProjectModel>,
    pub release: Arc<ReleaseModel>,
}

/// The project a prefix names right now.
///
/// Errors are text and say what would fix them — an id that is not on the board must come back as a
/// message, never as an empty result that reads like "there is nothing there".
pub fn resolve_project_by_prefix(
    board: &BoardInner,
    prefix: &str,
) -> Result<Arc<ProjectModel>, String> {
    let wanted = prefix.trim().to_uppercase();

    if wanted.is_empty() {
        return Err("no project given — pass a project prefix such as RMS".to_string());
    }

    board.get_project_by_prefix(&wanted).ok_or_else(|| {
        // Owned rather than borrowed: `projects()` hands back an Arc that would not outlive this
        // closure. Listing the real prefixes is the whole value of the message — "no such project" on
        // its own leaves the caller guessing whether it mistyped or the board is empty.
        let known: Vec<String> = board
            .projects()
            .iter()
            .map(|itm| itm.prefix.clone())
            .collect();

        if known.is_empty() {
            format!("no project with prefix '{wanted}'; there are no projects yet")
        } else {
            format!(
                "no project with prefix '{wanted}'; known prefixes: {}",
                known.join(", ")
            )
        }
    })
}

/// The task a handle names right now — `RMS-42`, or `RMS-000042`.
///
/// Resolves against the **current** holder of the prefix. A handle whose prefix has since moved to
/// another project therefore lands on that other project, which is correct for a live tool call and
/// misleading for a handle quoted from an old conversation; `tasks_resolve_id` exists to tell the two
/// apart.
pub fn resolve_task(board: &BoardInner, handle: &str) -> Result<ResolvedTask, String> {
    let parsed = parse_task_handle(handle)
        .ok_or_else(|| format!("'{handle}' is not a task id — expected something like RMS-42"))?;

    let project = resolve_project_by_prefix(board, &parsed.prefix)?;

    let task = board
        .get_task_including_deleted(&project.id, parsed.number)
        .ok_or_else(|| format!("no task {handle} on the {} board", project.prefix))?;

    Ok(ResolvedTask { project, task })
}

/// The goal a handle names right now — `RMS-G7`.
///
/// Only the marked spelling is accepted here. A bare `RMS-7` could be either kind, and a tool that was
/// asked to change a goal must not act on a task because the caller left the marker off; the lenient
/// reading belongs in search and `tasks_resolve_id`, where the answer is "here is what that means".
pub fn resolve_goal_by_handle(board: &BoardInner, handle: &str) -> Result<ResolvedGoal, String> {
    let parsed = parse_goal_handle(handle)
        .ok_or_else(|| format!("'{handle}' is not a goal id — expected something like RMS-G1"))?;

    let project = resolve_project_by_prefix(board, &parsed.prefix)?;

    let goal = board
        .get_goal_including_deleted(&project.id, parsed.number)
        .ok_or_else(|| format!("no goal {handle} on the {} board", project.prefix))?;

    Ok(ResolvedGoal { project, goal })
}

/// The release a handle names right now — `RMS-R12`.
///
/// Only the marked spelling, for the reason [`resolve_goal_by_handle`] gives: a tool asked to change a
/// release must not act on a task or a goal because the caller left the marker off.
///
/// A deleted release is found. The tools that take a handle are the ones that undelete it, and an id that
/// came back as "no such release" would be indistinguishable from a typo.
pub fn resolve_release_by_handle(
    board: &BoardInner,
    handle: &str,
) -> Result<ResolvedRelease, String> {
    let parsed = parse_release_handle(handle).ok_or_else(|| {
        format!("'{handle}' is not a release id — expected something like RMS-R1")
    })?;

    let project = resolve_project_by_prefix(board, &parsed.prefix)?;

    let release = board
        .get_release_including_deleted(&project.id, parsed.number)
        .ok_or_else(|| format!("no release {handle} on the {} board", project.prefix))?;

    Ok(ResolvedRelease { project, release })
}
