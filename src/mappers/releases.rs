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
            envs: envs_of_row(src),
            comments: src.comments.iter().map(|itm| itm.into()).collect(),
            created: src.created,
            updated: src.updated,
            deleted_moment: src.deleted_moment,
        }
    }
}

/// Where a stored release is out.
///
/// The column, whenever it has ever been written — an empty array included, which is a release somebody
/// took off every environment and must stay off them. Only a NULL falls through to what 0.2.0 stored
/// instead: such a row has not been written since environments existed, so its production mark is still
/// the only statement there is about where it is out.
fn envs_of_row(src: &ReleaseDto) -> Vec<String> {
    match &src.envs {
        Some(envs) => envs.clone(),
        None => ReleaseModel::envs_of_prod_mark(src.released_on_prod_moment.is_some()),
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
            // Always an array, so the row never again reads as one that predates environments.
            envs: Some(src.envs.clone()),
            // Emptied on every write: what it said has been carried into `envs` above.
            released_on_prod_moment: None,
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
            release_link: src.release_link.clone(),
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
            release_link: src.release_link.clone(),
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
                release_link: itm.release_link.clone(),
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
        envs: src.envs.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn row(
        envs: Option<Vec<&str>>,
        released_on_prod_moment: Option<DateTimeAsMicroseconds>,
    ) -> ReleaseDto {
        ReleaseDto {
            project_id: "p".to_string(),
            number: 12,
            title: "Releases".to_string(),
            description: String::new(),
            release_notes: String::new(),
            release_date: DateTimeAsMicroseconds::new(0),
            services: Vec::new(),
            comments: Vec::new(),
            envs: envs.map(|itm| itm.into_iter().map(str::to_string).collect()),
            released_on_prod_moment,
            created: DateTimeAsMicroseconds::new(0),
            updated: DateTimeAsMicroseconds::new(0),
            deleted_moment: None,
        }
    }

    /// A release 0.2.0 marked as out on production is a row with the old moment and no `envs` at all. It
    /// was live, so it has to load as live — as the label that replaced the mark.
    #[test]
    fn a_row_from_before_environments_carries_its_production_mark_over() {
        let marked: ReleaseModel = (&row(None, Some(DateTimeAsMicroseconds::new(5_000_000)))).into();
        assert_eq!(marked.envs, vec!["Prod"]);
        assert!(marked.is_on_env("prod"));

        let unmarked: ReleaseModel = (&row(None, None)).into();
        assert!(unmarked.envs.is_empty());
    }

    /// Once the row has been written by a build that knows environments, `envs` is the whole truth — and
    /// an EMPTY one most of all: that is a release taken off every environment, and reading the old mark
    /// back in would put it on production again at the next restart.
    #[test]
    fn a_row_that_has_environments_is_read_from_them_alone() {
        let moment = Some(DateTimeAsMicroseconds::new(5_000_000));

        let pulled_back: ReleaseModel = (&row(Some(Vec::new()), moment)).into();
        assert!(pulled_back.envs.is_empty());

        let on_dev: ReleaseModel = (&row(Some(vec!["Dev"]), moment)).into();
        assert_eq!(on_dev.envs, vec!["Dev"]);
    }

    /// The first write of a carried-over release settles it: the label goes into `envs`, the old column is
    /// emptied, and from then on there is one statement of where the release is out.
    #[test]
    fn writing_a_release_stores_its_environments_and_empties_the_old_mark() {
        let loaded: ReleaseModel = (&row(None, Some(DateTimeAsMicroseconds::new(5_000_000)))).into();

        let written: ReleaseDto = (&loaded).into();

        assert_eq!(written.envs, Some(vec!["Prod".to_string()]));
        assert!(written.released_on_prod_moment.is_none());

        let mut nowhere = loaded.clone();
        nowhere.envs.clear();

        let written: ReleaseDto = (&nowhere).into();
        assert_eq!(written.envs, Some(Vec::new()), "an array, never NULL again");
    }

    /// A service's link survives the trip to the row and back, and a row written before the field existed
    /// reads as a service with no link rather than failing to load.
    #[test]
    fn a_service_keeps_its_release_link_through_the_row() {
        let service = ServiceReleaseModel {
            microservice_id: "my-service".to_string(),
            version: "1.2.3".to_string(),
            git_hash: "099602e".to_string(),
            release_link: "https://github.com/o/r/releases/tag/1.2.3".to_string(),
            datetime: DateTimeAsMicroseconds::new(100 * 1_000_000),
            settings_update_note: String::new(),
            description: String::new(),
        };

        let stored: ServiceReleaseJsonModel = (&service).into();
        let read_back: ServiceReleaseModel = (&stored).into();
        assert_eq!(read_back, service);

        let before: ServiceReleaseJsonModel = serde_json::from_str(
            r#"{"microservice_id":"a","version":"1","git_hash":"abc1234","datetime_unix_seconds":5}"#,
        )
        .unwrap();

        assert_eq!(before.release_link, "");
    }
}
