use rust_extensions::date_time::DateTimeAsMicroseconds;
use task_manager_shared::releases::{ReleaseGoalResponse, ReleaseResponse, ServiceReleaseResponse};

use crate::board::{
    BoardInner, CommentModel, ProjectModel, ReleaseModel, ServiceReleaseModel, compose_goal_handle,
    compose_release_handle,
};
use crate::postgres::{ReleaseCommentJsonModel, ReleaseDto, ServiceReleaseJsonModel};

impl From<&ReleaseDto> for ReleaseModel {
    fn from(src: &ReleaseDto) -> Self {
        Self {
            project_id: src.project_id.clone(),
            number: src.number,
            title: src.title.clone(),
            description: src.description.clone(),
            release_notes: src.release_notes.clone(),
            date: src.release_date,
            services: src.services.iter().map(|itm| itm.into()).collect(),
            released_on_prod_moment: src.released_on_prod_moment,
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            deleted_moment: src.deleted_moment,
        }
    }
}

impl From<&ReleaseModel> for ReleaseDto {
    fn from(src: &ReleaseModel) -> Self {
        Self {
            project_id: src.project_id.clone(),
            number: src.number,
            title: src.title.clone(),
            description: src.description.clone(),
            release_notes: src.release_notes.clone(),
            release_date: src.date,
            services: src.services.iter().map(|itm| itm.into()).collect(),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            released_on_prod_moment: src.released_on_prod_moment,
            created: src.created,
            updated: src.updated,
            deleted_moment: src.deleted_moment,
        }
    }
}

impl From<&ReleaseCommentJsonModel> for CommentModel {
    fn from(src: &ReleaseCommentJsonModel) -> Self {
        Self {
            moment: DateTimeAsMicroseconds::new(src.moment_unix_seconds * 1_000_000),
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

impl From<&CommentModel> for ReleaseCommentJsonModel {
    fn from(src: &CommentModel) -> Self {
        Self {
            moment_unix_seconds: src.moment.unix_microseconds / 1_000_000,
            who: src.who.clone(),
            text: src.text.clone(),
        }
    }
}

impl From<&ServiceReleaseJsonModel> for ServiceReleaseModel {
    fn from(src: &ServiceReleaseJsonModel) -> Self {
        Self {
            microservice_id: src.microservice_id.clone(),
            version: src.version.clone(),
            git_hash: src.git_hash.clone(),
            datetime: DateTimeAsMicroseconds::new(src.datetime_unix_seconds * 1_000_000),
            settings_update_note: src.settings_update_note.clone(),
            description: src.description.clone(),
        }
    }
}

impl From<&ServiceReleaseModel> for ServiceReleaseJsonModel {
    fn from(src: &ServiceReleaseModel) -> Self {
        Self {
            microservice_id: src.microservice_id.clone(),
            version: src.version.clone(),
            git_hash: src.git_hash.clone(),
            datetime_unix_seconds: src.datetime.unix_microseconds / 1_000_000,
            settings_update_note: src.settings_update_note.clone(),
            description: src.description.clone(),
        }
    }
}

/// Memory -> wire.
///
/// `board` is read for the one thing a release does not know about itself: which goals list it. Their
/// names and colours are resolved here rather than left to the browser, because the goal a release
/// shipped is usually closed and aged off the screen by the time anybody reads the release — the snapshot
/// the browser holds would not have it.
pub fn release_to_response(
    src: &ReleaseModel,
    project: &ProjectModel,
    board: &BoardInner,
) -> ReleaseResponse {
    ReleaseResponse {
        id: compose_release_handle(&project.prefix, src.number),
        project: project.prefix.clone(),
        title: src.title.clone(),
        description: src.description.clone(),
        release_notes: src.release_notes.clone(),
        date_unix_seconds: src.date.unix_microseconds / 1_000_000,
        services: src
            .services
            .iter()
            .map(|itm| ServiceReleaseResponse {
                microservice_id: itm.microservice_id.clone(),
                version: itm.version.clone(),
                git_hash: itm.git_hash.clone(),
                datetime_unix_seconds: itm.datetime.unix_microseconds / 1_000_000,
                settings_update_note: itm.settings_update_note.clone(),
                description: itm.description.clone(),
            })
            .collect(),
        goals: board
            .goals_of_release(&src.project_id, src.number)
            .iter()
            .map(|goal| ReleaseGoalResponse {
                id: compose_goal_handle(&project.prefix, goal.number),
                name: goal.name.clone(),
                color: rust_extensions::AsStr::as_str(&goal.color).to_string(),
            })
            .collect(),
        released_on_prod_unix_seconds: src
            .released_on_prod_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
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
        deleted_unix_seconds: src
            .deleted_moment
            .map(|itm| itm.unix_microseconds / 1_000_000),
    }
}
