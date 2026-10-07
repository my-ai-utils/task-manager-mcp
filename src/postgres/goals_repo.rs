use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "goals";
pub const PK_NAME: &str = "goals_pk";

// One comment on a goal's thread, inside the goal row's `comments` jsonb.
//
// Deliberately its own type rather than a reuse of the task one: the two threads are the same shape
// today and have no reason to move together tomorrow, and a shared jsonb model would make a change to
// one silently rewrite the other's rows.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GoalCommentJsonModel {
    pub moment_unix_seconds: i64,
    pub who: String,
    pub text: String,
}

// One checklist item, inside the goal row's `subtasks` jsonb.
//
// Its own type rather than a reuse of the task one, exactly as the two comment models are kept apart: the
// two shapes are identical today and have no reason to move together tomorrow, and one shared jsonb model
// would make a change to a task's checklist silently rewrite every goal row.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct GoalSubtaskJsonModel {
    pub id: String,
    pub title: String,
    pub text: String,
    pub done: bool,
}

// A goal — the container work is done around. An epic.
//
// The primary key is `(project_id, number)`, and there is deliberately **no** column for the human
// handle: the counter is per project and a prefix moves between projects, so `RMS-G7` is a spelling of
// this row today and of nothing tomorrow. It is composed on read, exactly as a task's handle is.
//
// The number comes from the SAME counter as the project's task numbers, so no number names both a task
// and a goal, and a task can point at its goal with a bare number.
//
// There is no `status` column either. A goal has two states in this version and `close_moment` tells
// them apart on its own; a status beside it could disagree with it, and nothing would catch that. The
// wire reports a status — derived. Real columns for a goal are the second iteration, and the column
// arrives with them.
//
// It does not hold its tasks: the task row carries the goal number. That direction is what every read
// wants, since a task is drawn far more often than a goal is listed.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct GoalDto {
    // DEAD, and kept for exactly the reason `projects.columns` is kept. 0.1.7 shipped this table with a
    // generated text `id` as its primary key, before a goal was named `RMS-G7`; the deployed column is NOT
    // NULL and my-postgres cannot drop a column, so an insert that stopped listing it would fail. Written
    // as `project_id:number` rather than as anything meaningful, so it stays unique whichever primary key
    // the live table ends up with — see `dead_id`. Dropping it needs one
    // `ALTER TABLE goals DROP COLUMN id;` against the live database.
    pub id: String,
    #[primary_key(0)]
    pub project_id: String,
    #[primary_key(1)]
    pub number: i64,
    pub name: String,
    pub description: String,
    // A palette name — the same palette a task type is coloured from, because a board is one visual
    // system and two palettes would make a goal's colour mean nothing next to a type's.
    //
    // NULLABLE, and not because a goal can have no colour: the table already has rows, so a NOT NULL column
    // has to arrive with a default, and whether the schema generator quotes a text default is not a thing to
    // gamble a startup on. `None` reads as the default swatch, which is the same leniency an unrecognised
    // name gets — and every write puts a real name in.
    pub color: Option<String>,
    // How urgent the goal is, as the wire value of `Priority`. Nullable for the same reason `color` above is,
    // and `None` reads as Normal.
    pub priority: Option<String>,
    #[sql_type("jsonb")]
    #[json]
    pub comments: Vec<GoalCommentJsonModel>,
    // NULLABLE for the same reason `color` is: the column arrives on a table that already has rows, and
    // the generator would otherwise add it NOT NULL, which Postgres refuses. `None` reads as an empty
    // checklist, and every write puts a real array in.
    #[sql_type("jsonb")]
    #[json]
    pub subtasks: Option<Vec<GoalSubtaskJsonModel>>,
    // Ids of the documents this goal references. Same shape, same rules and same reasons as a task's — see
    // `TaskDto::documents`. A goal is where a decision gets written down, so it is the more likely of the
    // two to point at a specification.
    #[sql_type("jsonb")]
    #[json]
    pub documents: Option<Vec<String>>,
    // Numbers of the releases this goal went out in — rows of `releases` on this same project. Numbers
    // and not handles for the reason `tasks.depends_on` holds numbers: a prefix moves between projects.
    //
    // The link is stored HERE rather than on the release, so a release stays a plain record of what
    // shipped and attaching one is a change to the goal — one atomic upsert of this row.
    //
    // NULLABLE for the same reason `subtasks` and `documents` are: the column arrives on a table that
    // already has rows. `None` reads as no releases, and every write puts a real array in.
    #[sql_type("jsonb")]
    #[json]
    pub releases: Option<Vec<i64>>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
    // When the goal was closed; NULL while it is open. Nullable rather than defaulted, because "still
    // open" and "closed at the epoch" are different facts and only one of them is true of a live goal.
    #[sql_type("timestamp")]
    pub close_moment: Option<DateTimeAsMicroseconds>,
    // When the goal was deleted; NULL for one that is not. Same shape and same reasons as a task's — see
    // `TaskDto::deleted_moment`.
    //
    // Its arrival is what the goals_update tool description meant by "one that should never have existed
    // stays until deletion exists".
    #[sql_type("timestamp")]
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

pub struct GoalsRepo {
    postgres: MyPostgres,
}

impl GoalsRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<GoalDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every goal. Called once at startup — after that the answer lives in memory.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<GoalDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("goals: query_rows get_all failed")
    }

    /// Write a goal whole. There is no delete: a goal is closed, never removed — see `GOAL-HANDOFF.md`.
    pub async fn upsert(&self, row: &GoalDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("goals: insert_or_update_db_entity failed");
    }
}

/// The value written into the dead `id` column.
///
/// A goal's identity is `(project_id, number)`; this only has to be unique and non-null, because the
/// deployed 0.1.7 table still carries the column — possibly still as its primary key, if the schema
/// verifier has not recreated it. Composing it from both halves means the row is unique under EITHER
/// definition, so the upsert behaves the same before and after that recreation happens.
pub fn dead_id(project_id: &str, number: i64) -> String {
    format!("{project_id}:{number}")
}
