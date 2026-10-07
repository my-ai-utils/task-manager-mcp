use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "releases";
pub const PK_NAME: &str = "releases_pk";

// One microservice of a release, inside the release row's `services` jsonb.
//
// Rides on the row rather than living in a table of its own for the reason a task's checklist does: adding
// a service to a release is then one atomic upsert of one row, and the in-memory copy is replaced whole
// rather than reconciled against a half-applied change. Nothing is ever asked of these across releases
// that the in-memory board does not answer.
//
// The two notes carry `#[serde(default)]` because they are prose that may be added to later: a row written
// by a build that did not know a field must still load.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ServiceReleaseJsonModel {
    pub microservice_id: String,
    pub version: String,
    pub git_hash: String,
    pub datetime_unix_seconds: i64,
    #[serde(default)]
    pub settings_update_note: String,
    #[serde(default)]
    pub description: String,
}

// One comment on a release's thread, inside the release row's `comments` jsonb.
//
// Its own type rather than a reuse of the task's or the goal's, as those two are kept apart from each
// other: the three threads are the same shape today and have no reason to move together tomorrow, and a
// shared jsonb model would make a change to one silently rewrite the others' rows.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ReleaseCommentJsonModel {
    pub moment_unix_seconds: i64,
    pub who: String,
    pub text: String,
}

// A release — the record that a feature went out, and in what.
//
// The primary key is `(project_id, number)`, exactly as a task's and a goal's is, and for the same reason
// there is no column for the human handle: the number comes from the project's ONE counter, and `RMS-R7`
// is composed on read from whatever prefix the project carries today.
//
// It holds no reference to a goal. The goal row lists the releases it went out in (`goals.releases`), so
// this table is a plain log of what shipped and a release can be written before anybody has decided which
// goal it belongs under.
//
// `release_date` and not `date`: the schema generator does not quote identifiers — see `TaskDto` — and a
// column named after a type is exactly the kind of thing that is fine until the day it is not.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct ReleaseDto {
    #[primary_key(0)]
    pub project_id: String,
    #[primary_key(1)]
    pub number: i64,
    pub title: String,
    pub description: String,
    pub release_notes: String,
    // The date of the release as its author gave it. What a list of releases is ordered by.
    #[sql_type("timestamp")]
    pub release_date: DateTimeAsMicroseconds,
    #[sql_type("jsonb")]
    #[json]
    pub services: Vec<ServiceReleaseJsonModel>,
    // The thread. NOT NULL, unlike the jsonb columns that were added to `goals` and `tasks` after they had
    // rows: this column is as old as its table, so there was never a populated table to add it to.
    #[sql_type("jsonb")]
    #[json]
    pub comments: Vec<ReleaseCommentJsonModel>,
    // When the release was marked as out on production; NULL while it is not. A moment rather than a
    // flag for the reason `deleted_moment` is: "on prod" and "since when" are one fact, and two columns
    // for it could disagree.
    #[sql_type("timestamp")]
    pub released_on_prod_moment: Option<DateTimeAsMicroseconds>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
    // When the release was deleted; NULL for one that is not. Same shape and same reasons as a task's —
    // see `TaskDto::deleted_moment`.
    #[sql_type("timestamp")]
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

pub struct ReleasesRepo {
    postgres: MyPostgres,
}

impl ReleasesRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<ReleaseDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every release of every project. Called once at startup — after that the answer lives in memory.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<ReleaseDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("releases: query_rows get_all failed")
    }

    /// Write a release whole. There is no delete: one recorded by mistake is flagged, never removed.
    pub async fn upsert(&self, row: &ReleaseDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("releases: insert_or_update_db_entity failed");
    }
}
