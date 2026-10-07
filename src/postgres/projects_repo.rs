use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "projects";
pub const PK_NAME: &str = "projects_pk";

// One column of a project's board, inside the project row's `columns` jsonb.
//
// The two anchors (`todo`, `done`) are NOT stored here — they exist in every project by definition,
// so persisting them would only create a way for a project to be missing one.
//
// `column_order` rather than `order`: the schema generator does not quote identifiers, and `order`
// is a reserved word. This one is a JSON key rather than a column, but keeping the two spellings
// identical avoids a trap the next person would have to rediscover.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectColumnJsonModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub column_order: i32,
}

// One kind of work, inside the project row's `kinds` jsonb. `color` is a
// `task_manager_shared::kind_color::KindColor` wire value; an unrecognised one reads as the default
// swatch rather than failing the load.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectKindJsonModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: String,
}

// One connected GitHub repository, inside the project row's `github_connections` jsonb.
//
// THE KEY IS NOT HERE, and that absence is the feature. A token lives in the process's memory for as
// long as the process does and is written nowhere — not to this row, not to the settings service, not
// to a log. So a database dump carries no credential, and the cost is that a private repository stops
// mirroring after a restart until somebody types the key again. That trade was chosen deliberately.
//
// `branch` and `repo_path` are empty rather than absent when they mean "the default branch" and "the
// whole repository": a jsonb written by an older build must read back as something, and an empty
// string is a value every reader already handles.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ProjectGithubConnectionJsonModel {
    pub name: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub repo_path: String,
}

// A project. Read once at startup and held in memory from then on; written whenever setup changes.
//
// `last_task_number` is the project's own task counter, persisted so a restart does not re-issue a
// live number. It is only ever the high-water mark — deleting a task does not lower it.
//
// `prefix_history` is every prefix this project has carried, excluding the current one. It exists so
// `tasks_resolve_id` can answer what an old `RMS-42` used to mean after the project renamed away
// from `RMS` and somebody else took it over.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct ProjectDto {
    #[primary_key(0)]
    pub id: String,
    pub name: String,
    pub description: String,
    #[db_index(id: 0, index_name: "projects_prefix_idx", is_unique: true, order: "ASC")]
    pub prefix: String,
    #[sql_type("jsonb")]
    #[json]
    pub prefix_history: Vec<String>,
    // Which column template this project follows. `None` means it follows none and its board is
    // Todo -> Done.
    pub column_template_id: Option<String>,
    // Which task-type template this project follows. `None` means it follows none and has no task types,
    // which is legitimate — a type is optional on a task.
    pub kind_template_id: Option<String>,
    // DEAD. Columns moved into `column_templates`; this is written empty and never read. `kinds` below
    // is dead for the same reason, having moved into `kind_templates`.
    //
    // Kept in the DTO rather than removed because the deployed table has the column NOT NULL, and an
    // insert that stopped listing it would fail. Dropping it needs one `ALTER TABLE projects DROP
    // COLUMN columns` against the live database — see TODO.md.
    #[sql_type("jsonb")]
    #[json]
    pub columns: Vec<ProjectColumnJsonModel>,
    #[sql_type("jsonb")]
    #[json]
    pub kinds: Vec<ProjectKindJsonModel>,
    pub last_task_number: i64,
    // How many days finished work stays on the board. NULL means the built-in default of seven, which is
    // what every project did before this was configurable — so an existing row keeps its behaviour
    // without being rewritten, and the column can be added to a live table without a backfill.
    pub archive_days: Option<i32>,
    // The GitHub repositories mirrored into this project's documents under `github/`. NULL means none,
    // which is what every row that predates the feature holds — `TableSchema` can add a column to a live
    // table but cannot tighten NULL to NOT NULL, so nullable is the only shape that deploys without a
    // backfill.
    #[sql_type("jsonb")]
    #[json]
    pub github_connections: Option<Vec<ProjectGithubConnectionJsonModel>>,
    // When the project was archived — put away, not deleted; NULL for one that is not.
    //
    // A moment rather than a flag, for the same reason `deleted_moment` on a task is one: "archived" and
    // "archived when" are one fact, and two columns for it can disagree. Nothing reads the moment today,
    // and that is fine — a flag can never become a moment afterwards, whereas a moment is already both.
    //
    // Nullable, and it has to be: `TableSchema` can add a column to a live table but cannot tighten NULL
    // to NOT NULL, so this is the only shape that deploys without a backfill — and NULL is exactly what
    // every row written before this column means anyway.
    //
    // Not to be confused with `archive_days` above, which is about a finished TASK ageing off the board.
    #[sql_type("timestamp")]
    pub archived_moment: Option<DateTimeAsMicroseconds>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
}

pub struct ProjectsRepo {
    postgres: MyPostgres,
}

impl ProjectsRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<ProjectDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every project. Called once at startup — after that the answer lives in memory.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<ProjectDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            // `NoneWhereModel::new()` *is* the `None` — it returns Option<&'static Self>, so it is
            // passed straight through rather than wrapped in `Some`.
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("projects: query_rows get_all failed")
    }

    pub async fn upsert(&self, row: &ProjectDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("projects: insert_or_update_db_entity failed");
    }
}
