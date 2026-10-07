use task_manager_shared::goals::GoalResponse;

use crate::board::{
    BoardInner, CommentModel, GoalModel, ProjectModel, SubtaskModel, compose_goal_handle,
};
use crate::postgres::{GoalCommentJsonModel, GoalDto, GoalSubtaskJsonModel};

impl From<&GoalDto> for GoalModel {
    fn from(src: &GoalDto) -> Self {
        Self {
            project_id: src.project_id.clone(),
            number: src.number,
            name: src.name.clone(),
            description: src.description.clone(),
            // Unknown reads as the default swatch rather than failing the load, the same leniency a task
            // type's colour gets.
            color: task_manager_shared::kind_color::KindColor::parse_or_default(
                src.color.as_deref().unwrap_or_default(),
            ),
            // NULL is a goal written before priorities existed; it reads as Normal, which is what an unranked
            // goal is.
            priority: task_manager_shared::priority::Priority::parse_or_default(
                src.priority.as_deref().unwrap_or_default(),
            ),
            // A NULL column is a goal written before checklists existed, and it reads as having none.
            subtasks: src
                .subtasks
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|itm| itm.into())
                .collect(),
            // A NULL column is a goal written before documents existed, and it reads as referencing none.
            documents: src.documents.clone().unwrap_or_default(),
            // A NULL column is a goal written before releases existed, and it reads as having shipped in
            // none.
            releases: src.releases.clone().unwrap_or_default(),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            close_moment: src.close_moment,
            deleted_moment: src.deleted_moment,
        }
    }
}

impl From<&GoalModel> for GoalDto {
    fn from(src: &GoalModel) -> Self {
        Self {
            // Dead column, written because the deployed table still has it NOT NULL. See `GoalDto`.
            id: crate::postgres::dead_id(&src.project_id, src.number),
            project_id: src.project_id.clone(),
            number: src.number,
            name: src.name.clone(),
            description: src.description.clone(),
            color: Some(rust_extensions::AsStr::as_str(&src.color).to_string()),
            priority: Some(rust_extensions::AsStr::as_str(&src.priority).to_string()),
            // Always a real array, even when empty — the column is nullable only so it could be added to a
            // populated table.
            subtasks: Some(src.subtasks.iter().map(|itm| itm.into()).collect()),
            // Always a real array, for the same reason the checklist is.
            documents: Some(src.documents.clone()),
            // And again: nullable only so the column could arrive on a populated table.
            releases: Some(src.releases.clone()),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            close_moment: src.close_moment,
            deleted_moment: src.deleted_moment,
        }
    }
}

impl From<&GoalSubtaskJsonModel> for SubtaskModel {
    fn from(src: &GoalSubtaskJsonModel) -> Self {
        Self {
            id: src.id.clone(),
            title: src.title.clone(),
            text: src.text.clone(),
            done: src.done,
        }
    }
}

impl From<&SubtaskModel> for GoalSubtaskJsonModel {
    fn from(src: &SubtaskModel) -> Self {
        Self {
            id: src.id.clone(),
            title: src.title.clone(),
            text: src.text.clone(),
            done: src.done,
        }
    }
}

impl From<&GoalCommentJsonModel> for CommentModel {
    fn from(src: &GoalCommentJsonModel) -> Self {
        Self {
            moment: rust_extensions::date_time::DateTimeAsMicroseconds::new(
                src.moment_unix_seconds * 1_000_000,
            ),
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

impl From<&CommentModel> for GoalCommentJsonModel {
    fn from(src: &CommentModel) -> Self {
        Self {
            moment_unix_seconds: src.moment.unix_microseconds / 1_000_000,
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

/// Memory -> wire.
///
/// Three things here come from outside the goal, which is why the project and the board are passed in:
/// the handle is composed from the project's current prefix; the progress is counted against the whole
/// board, which a goal knows nothing about; and the releases are resolved from the numbers the goal
/// stores — a release is a project-level record, so the goal holds only where to find it.
///
/// Taking the board rather than the three results is deliberate. Every caller used to count the progress
/// itself and hand it over, and a second thing computed at each call site is a second thing one of them
/// forgets.
pub fn goal_to_response(src: &GoalModel, project: &ProjectModel, board: &BoardInner) -> GoalResponse {
    let prefix = project.prefix.as_str();
    let (tasks_amount, done_amount) = board.goal_progress(&src.project_id, src.number);

    GoalResponse {
        id: compose_goal_handle(prefix, src.number),
        project: prefix.to_string(),
        name: src.name.clone(),
        description: src.description.clone(),
        color: rust_extensions::AsStr::as_str(&src.color).to_string(),
        priority: rust_extensions::AsStr::as_str(&src.priority).to_string(),
        status: src.status().to_string(),
        tasks_amount: tasks_amount as i32,
        done_amount: done_amount as i32,
        subtasks: super::subtasks_to_response(&src.subtasks),
        // Ids only — see `task_to_response`.
        documents: src.documents.clone(),
        // Whole releases, newest first — deleted ones already left out by the board.
        releases: board
            .releases_of_goal(src)
            .iter()
            .map(|release| super::release_to_response(release, project, board))
            .collect(),
        comments: src
            .comments
            .iter()
            .map(|itm| task_manager_shared::tasks::TaskCommentResponse {
                moment_unix_seconds: itm.moment.unix_microseconds / 1_000_000,
                who: itm.who.clone(),
                text: itm.text.clone(),
            })
            .collect(),
        created_unix_seconds: src.created.unix_microseconds / 1_000_000,
        updated_unix_seconds: src.updated.unix_microseconds / 1_000_000,
        closed_unix_seconds: src
            .close_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
        deleted_unix_seconds: src
            .deleted_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
    }
}
