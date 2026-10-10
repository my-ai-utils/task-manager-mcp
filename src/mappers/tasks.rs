use task_manager_shared::subtasks::SubtaskResponse;
use task_manager_shared::tasks::{
    TaskCommentResponse, TaskGhActionResponse, TaskLinkResponse, TaskResponse,
};

use crate::board::{
    BoardInner, CommentModel, GhActionModel, ProjectModel, SubtaskModel, TaskModel,
    compose_task_handle, parse_task_handle,
};
use crate::postgres::{
    TaskCommentJsonModel, TaskDto, TaskGhActionJsonModel, TaskSubtaskJsonModel,
};

impl From<&TaskCommentJsonModel> for CommentModel {
    fn from(src: &TaskCommentJsonModel) -> Self {
        Self {
            moment: rust_extensions::date_time::DateTimeAsMicroseconds::new(
                src.moment_unix_seconds * 1_000_000,
            ),
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

impl From<&CommentModel> for TaskCommentJsonModel {
    fn from(src: &CommentModel) -> Self {
        Self {
            moment_unix_seconds: src.moment.unix_microseconds / 1_000_000,
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

impl From<&TaskGhActionJsonModel> for GhActionModel {
    fn from(src: &TaskGhActionJsonModel) -> Self {
        Self {
            url: src.url.clone(),
            title: src.title.clone(),
            moment: rust_extensions::date_time::DateTimeAsMicroseconds::new(
                src.moment_unix_seconds * 1_000_000,
            ),
        }
    }
}

impl From<&GhActionModel> for TaskGhActionJsonModel {
    fn from(src: &GhActionModel) -> Self {
        Self {
            url: src.url.clone(),
            title: src.title.clone(),
            moment_unix_seconds: src.moment.unix_microseconds / 1_000_000,
        }
    }
}

impl From<&TaskSubtaskJsonModel> for SubtaskModel {
    fn from(src: &TaskSubtaskJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            title: src.title.clone(),
            text: src.text.clone(),
            done: src.done,
        }
    }
}

impl From<&SubtaskModel> for TaskSubtaskJsonModel {
    fn from(src: &SubtaskModel) -> Self {
        Self {
            id: src.id.clone(),
            title: src.title.clone(),
            text: src.text.clone(),
            done: src.done,
        }
    }
}

/// Memory -> wire, for one checklist. Shared by a task and a goal: the item is the same thing on both, and
/// the module it comes from belongs to neither.
pub fn subtasks_to_response(src: &[SubtaskModel]) -> Vec<SubtaskResponse> {
    src.iter()
        .map(|itm| SubtaskResponse {
            id: itm.id.clone(),
            title: itm.title.clone(),
            text: itm.text.clone(),
            done: itm.done,
        })
        .collect()
}

impl From<&TaskDto> for TaskModel {
    fn from(src: &TaskDto) -> Self {
        Self {
            project_id: src.project_id.clone(),
            number: src.number,
            text: src.task_text.clone(),
            status: src.status.clone(),
            // A NULL column is a task written before priorities existed, and an unrecognised value is one
            // written by a build that knew one more — both read as Normal rather than failing the load.
            priority: task_manager_shared::priority::Priority::parse_or_default(
                src.priority.as_deref().unwrap_or_default(),
            ),
            kind: src.kind.clone(),
            goal_number: src.goal_number,
            assignee: src.assignee.clone(),
            labels: src.labels.clone(),
            depends_on: src.depends_on.clone(),
            // A NULL column is a task written before checklists existed, and it reads as having none.
            subtasks: src
                .subtasks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|itm| itm.into())
                .collect(),
            // A NULL column is a task written before documents existed, and it reads as referencing none.
            documents: src.documents.clone().unwrap_or_default(),
            // And a NULL here is a task written before builds were recorded — it reads as having produced
            // none, which of a task that landed before the column existed is simply true.
            gh_actions: src
                .gh_actions
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|itm| itm.into())
                .collect(),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            start_moment: src.start_moment,
            close_moment: src.close_moment,
            deleted_moment: src.deleted_moment,
        }
    }
}

impl From<&TaskModel> for TaskDto {
    fn from(src: &TaskModel) -> Self {
        Self {
            project_id: src.project_id.clone(),
            number: src.number,
            task_text: src.text.clone(),
            status: src.status.clone(),
            // Always a real value on the way out, even for a task nobody ranked: the column is nullable only
            // so it could be added to a populated table, not so a write has two ways to say "Normal".
            priority: Some(rust_extensions::AsStr::as_str(&src.priority).to_string()),
            kind: src.kind.clone(),
            goal_number: src.goal_number,
            assignee: src.assignee.clone(),
            labels: src.labels.clone(),
            depends_on: src.depends_on.clone(),
            // Always a real array on the way out, even when it is empty: the column is nullable only so it
            // could be added to a populated table, not so a write has two ways to say "no checklist".
            subtasks: Some(src.subtasks.iter().map(|itm| itm.into()).collect()),
            // Always a real array, for the same reason the checklist is.
            documents: Some(src.documents.clone()),
            gh_actions: Some(src.gh_actions.iter().map(|itm| itm.into()).collect()),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            start_moment: src.start_moment,
            close_moment: src.close_moment,
            deleted_moment: src.deleted_moment,
        }
    }
}

/// Memory -> wire, resolving everything a reader should not have to work out for itself.
///
/// Four things happen here that cannot happen in a plain `From`, because all four need the rest of
/// the board:
///
/// * the handle is **composed** from the project's current prefix — it is not stored;
/// * `depends_on` numbers become handles, so a client never sees an internal number;
/// * `blocked` / `blocks` are derived against the current statuses of every other task;
/// * an unknown status reads as Todo and an unknown kind as no kind, so a deleted column or kind
///   renders instead of breaking the read.
pub fn task_to_response(
    task: &TaskModel,
    project: &ProjectModel,
    board: &BoardInner,
) -> TaskResponse {
    let goal = board.effective_goal(task);

    let assignee_name = task
        .assignee
        .as_ref()
        .and_then(|itm| board.display_name_of(itm));

    // Read once: it is a scan of the project's tasks, and both `blocks` and the statuses beside it need
    // the same answer.
    let blocks = board.blocks(&task.project_id, task.number);

    TaskResponse {
        id: compose_task_handle(&project.prefix, task.number),
        project: project.prefix.clone(),
        text: task.text.clone(),
        status: project.effective_status(&task.status),
        priority: rust_extensions::AsStr::as_str(&task.priority).to_string(),
        kind: project.effective_kind(task.kind.as_deref()),
        // Resolved and composed, so a task whose goal number names nothing reads as standalone rather
        // than as a dangling number nobody can look up.
        goal: goal
            .as_ref()
            .map(|itm| crate::board::compose_goal_handle(&project.prefix, itm.number)),
        goal_name: goal.as_ref().map(|itm| itm.name.clone()),
        goal_color: goal
            .as_ref()
            .map(|itm| rust_extensions::AsStr::as_str(&itm.color).to_string()),
        assignee: task.assignee.clone(),
        assignee_name,
        labels: task.labels.clone(),
        depends_on: task
            .depends_on
            .iter()
            .map(|number| compose_task_handle(&project.prefix, *number))
            .collect(),
        blocks: blocks
            .iter()
            .map(|number| compose_task_handle(&project.prefix, *number))
            .collect(),
        link_statuses: link_statuses(task, &blocks, project, board),
        blocked: board.is_blocked(task),
        subtasks: subtasks_to_response(&task.subtasks),
        // The ids only. Resolving them means reading Postgres, which this function cannot do and a board
        // read must not do — the browser asks for a document when somebody opens one, and draws the count
        // from this list in the meantime.
        documents: task.documents.clone(),
        // Whole, unlike the documents above: there is nothing to resolve — the entry is the link and the
        // name — so the dialog draws it straight out of the board snapshot.
        gh_actions: task
            .gh_actions
            .iter()
            .map(|itm| TaskGhActionResponse {
                url: itm.url.clone(),
                title: itm.title.clone(),
                moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
            })
            .collect(),
        comments: task
            .comments
            .iter()
            .map(|itm| TaskCommentResponse {
                moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
                who: itm.who.clone(),
                text: itm.text.clone(),
            })
            .collect(),
        created_unix_seconds: task.created.unix_microseconds / 1_000_000,
        updated_unix_seconds: task.updated.unix_microseconds / 1_000_000,
        started_unix_seconds: task
            .start_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
        closed_unix_seconds: task
            .close_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
        // Sent rather than filtered out here: the board snapshot carries deleted work so that SEARCHING can
        // find it, and the screen hides it. Filtering on the server would make a deleted task unfindable,
        // which is the one thing the flag exists to prevent.
        deleted_unix_seconds: task
            .deleted_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
    }
}

/// What every task on either end of a dependency is doing.
///
/// Both directions in one list rather than two: the reader asks the same question of a blocker and of
/// something blocked — is it done — and an id appears in only one of the two lists anyway, because a task
/// that both waited on and blocked the same task would be a cycle.
///
/// Dependencies never cross projects, which is what makes the lookup a project-local one and this a scan of
/// tasks already in memory rather than a query.
fn link_statuses(
    task: &TaskModel,
    blocks: &[i64],
    project: &ProjectModel,
    board: &BoardInner,
) -> Vec<TaskLinkResponse> {
    task.depends_on
        .iter()
        .chain(blocks.iter())
        .filter_map(|number| {
            // No entry for a number that names no task. It is the case that keeps `blocked` true — a typo
            // or a deleted blocker — and inventing a status for it would hide exactly that.
            let linked = board.get_task(&task.project_id, *number)?;

            Some(TaskLinkResponse {
                id: compose_task_handle(&project.prefix, *number),
                status: project.effective_status(&linked.status),
            })
        })
        .collect()
}

/// Read a `depends_on` entry written by a caller: a handle (`RMS-7`) or a bare number (`7`).
///
/// Bare numbers are accepted because within one project they are unambiguous and a caller editing a
/// dependency list should not have to retype the prefix. A handle whose prefix belongs to a
/// *different* project is refused rather than silently reinterpreted — dependencies do not cross
/// projects, and quietly treating `OTHER-7` as "task 7 here" would create one that looks fine.
pub fn parse_dependency(src: &str, project: &ProjectModel) -> Result<i64, String> {
    let src = src.trim();

    if let Ok(number) = src.parse::<i64>() {
        if number > 0 {
            return Ok(number);
        }
        return Err(format!("'{src}' is not a task number"));
    }

    let parsed = parse_task_handle(src)
        .ok_or_else(|| format!("'{src}' is not a task id — expected something like RMS-42"))?;

    if parsed.prefix != project.prefix {
        return Err(format!(
            "'{src}' belongs to another project; a task can only depend on tasks of {}",
            project.prefix
        ));
    }

    Ok(parsed.number)
}
