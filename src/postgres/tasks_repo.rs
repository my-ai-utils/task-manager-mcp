use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "tasks";
pub const PK_NAME: &str = "tasks_pk";

// One comment, inside the task row's `comments` jsonb.
//
// The thread rides on the task rather than living in its own table so that adding a comment is one
// atomic upsert of one row. A second table would mean two writes with no transaction around them,
// and the in-memory copy would have to be reconciled against a half-applied change.
//
// `who` is an email or the literal `AI`, unvalidated on purpose: MCP has no session to derive an
// author from, and an author whose user row was later removed still has to render.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskCommentJsonModel {
    pub moment_unix_seconds: i64,
    pub who: String,
    pub text: String,
}

// One checklist item, inside the task row's `subtasks` jsonb.
//
// Rides on the row for the same reason the thread does: ticking an item is then one atomic upsert of one
// row, and the in-memory copy is replaced whole rather than reconciled against a half-applied change.
//
// `id` is a `SortableId` generated when the item is created. It is not a task number — nothing here comes
// out of the project's counter, because a checklist item is not a task and never gets a handle.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskSubtaskJsonModel {
    pub id: String,
    pub title: String,
    pub text: String,
    pub done: bool,
}

// One build that came out of the task, inside the task row's `gh_actions` jsonb.
//
// Rides on the row for the same reason the thread and the checklist do: attaching a build link is then one
// atomic upsert of one row, and the links travel in the board snapshot, so the dialog draws them without a
// request.
//
// `url` is the identity — there is no id here, because the run already has one and it is IN the url.
// Removing a link names the url, and adding one that is already there does not duplicate it.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TaskGhActionJsonModel {
    pub url: String,
    pub title: String,
    pub moment_unix_seconds: i64,
}

// One task.
//
// The primary key is `(project_id, number)`, and there is deliberately **no** column for the human
// id: the counter is per project and a prefix can move between projects, so two projects can both
// produce `RMS-1`. The displayed id is composed on read from the project's current prefix. For the
// same reason `depends_on` holds numbers — storing `"RMS-7"` would break every dependency in the
// project the moment its prefix changed.
//
// `task_text` and `column_order`-style names avoid Postgres keywords: the schema generator does not
// quote identifiers, and a column it creates but cannot then find in the catalogue gets recreated on
// every startup.
//
// `status` and `kind` are ids of the owning project's configured columns and kinds, with no foreign
// key to either. A column is deleted freely and its tasks keep their stored status, reading as Todo
// from then on — a constraint would have made that impossible instead of merely lenient.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct TaskDto {
    #[primary_key(0)]
    pub project_id: String,
    #[primary_key(1)]
    pub number: i64,
    pub task_text: String,
    pub status: String,
    // How urgent it is, as the wire value of `Priority`. NULLABLE for the same reason `goals.color` is: the
    // column arrives on a table that already has rows, and the generator takes a column's nullability from
    // the Rust type — a non-Option field would emit `add priority text not null`, which Postgres refuses on a
    // populated table. NULL reads as Normal, which is exactly what an unranked task is, and every write puts
    // a real value in.
    pub priority: Option<String>,
    pub kind: Option<String>,
    // Which goal this task is part of, by the goal's number in this same project. `None` for a
    // standalone task. A number rather than a handle for the same reason `depends_on` holds numbers.
    pub goal_number: Option<i64>,
    pub assignee: Option<String>,
    #[sql_type("jsonb")]
    #[json]
    pub labels: Vec<String>,
    #[sql_type("jsonb")]
    #[json]
    pub depends_on: Vec<i64>,
    #[sql_type("jsonb")]
    #[json]
    pub comments: Vec<TaskCommentJsonModel>,
    // NULLABLE, and not because a task can have no checklist — an empty list says that perfectly well.
    // The table already has rows, and the schema generator adds a new column with the nullability it
    // derives from the Rust type: a non-Option field would emit `alter table … add subtasks jsonb not
    // null`, which Postgres refuses on a populated table. `None` reads as an empty checklist, and every
    // write puts a real array in. Same arrangement and same reason as `goals.color`.
    #[sql_type("jsonb")]
    #[json]
    pub subtasks: Option<Vec<TaskSubtaskJsonModel>>,
    // Ids of the documents this task references — `SortableId`s, exactly as `documents.id` stores them.
    //
    // Ids and not paths, which is the whole reason a document's id never changes: a document that moves
    // keeps every reference to it, and a reference that stored `docs/design/system.md` would have gone
    // stale the moment somebody tidied the tree.
    //
    // Rides on the task row rather than living in a join table for the same reason the thread and the
    // checklist do — attaching a document is then one atomic upsert of one row. It also means the ids
    // travel in the board snapshot, so a card can show HOW MANY documents a task has without a single
    // request; the documents themselves are fetched only when somebody opens one.
    //
    // Nothing here is cleaned up when a document is deleted. That is deliberate: deletion moves the
    // document to the trash, restoring it is one MCP call, and a reference silently removed would not come
    // back. An id that resolves to nothing reads as "in the trash" rather than as an error.
    //
    // NULLABLE for the same reason `subtasks` is: the column arrives on a populated table, where Postgres
    // refuses a NOT NULL addition. `None` reads as no references, and every write puts a real array in.
    #[sql_type("jsonb")]
    #[json]
    pub documents: Option<Vec<String>>,
    // The GitHub Actions runs this task produced, oldest first — see `TaskGhActionJsonModel`.
    //
    // Written by whoever did the work, never fetched: this service does not talk to GitHub, so what is stored
    // is what a caller said, and nothing here goes stale because nothing here is a copy of anything.
    //
    // NULLABLE for the same reason `subtasks` and `documents` are: the column arrives on a populated table,
    // where Postgres refuses a NOT NULL addition. `None` reads as no builds, and every write puts a real
    // array in.
    #[sql_type("jsonb")]
    #[json]
    pub gh_actions: Option<Vec<TaskGhActionJsonModel>>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
    #[sql_type("timestamp")]
    pub updated: DateTimeAsMicroseconds,
    // When the task left Todo; NULL while it is there — see `TaskModel::start_moment`. Nullable because
    // the column arrives on a populated table, where every task written before it reads as "not recorded".
    #[sql_type("timestamp")]
    pub start_moment: Option<DateTimeAsMicroseconds>,
    // When the task landed in Done; NULL whenever it is not there. Nullable rather than defaulted,
    // because "never closed" and "closed at the epoch" are different facts and only one of them is true
    // of a task in Todo.
    #[sql_type("timestamp")]
    pub close_moment: Option<DateTimeAsMicroseconds>,
    // When the task was deleted; NULL for one that is not.
    //
    // **A moment rather than a flag, and the row rather than a second table.** The moment because "deleted"
    // and "deleted when" are one fact and two columns for it can disagree — the same reason `close_moment` is
    // shaped this way. The row because a deleted task must still be FINDABLE: searching for its id has to
    // turn it up and say it is gone, which a task moved out of the table could not do.
    //
    // Nullable, so it could be added to a populated table — and because "never deleted" and "deleted at the
    // epoch" are different facts.
    #[sql_type("timestamp")]
    pub deleted_moment: Option<DateTimeAsMicroseconds>,
}

pub struct TasksRepo {
    postgres: MyPostgres,
}

impl TasksRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<TaskDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every task of every project. Called once at startup — the board is held in memory after that.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<TaskDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("tasks: query_rows get_all failed")
    }

    pub async fn upsert(&self, row: &TaskDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("tasks: insert_or_update_db_entity failed");
    }
}
