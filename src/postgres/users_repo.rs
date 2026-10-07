use std::sync::Arc;
use std::time::Duration;

use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "users";
pub const PK_NAME: &str = "users_pk";

// One person.
//
// The email is the identity and the primary key: it is what Google returns, what a task's `assignee`
// carries and what a comment's `who` carries. A person whose address changes is a new row on purpose
// — their old assignments keep pointing at the address that made them, which is a record of what
// happened rather than a live pointer.
//
// `admin` is only half the answer to "is this person an admin": the `admins` list in the service
// settings is additive on top of it. That is what lets the first person sign in to an empty
// database, where by definition no row here says admin.
//
// `disabled` denies sign-in and nothing else. Assignments and comment authorship are untouched — a
// disabled colleague's name still renders on the stickers they were given.
//
// The table is named `users`, not `user`: `user` is a reserved word and the schema generator does not
// quote identifiers.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct UserDto {
    #[primary_key(0)]
    pub email: String,
    pub name: String,
    pub admin: bool,
    pub disabled: bool,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
}

pub struct UsersRepo {
    postgres: MyPostgres,
}

impl UsersRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<UserDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// The whole roster. Called once at startup.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<UserDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("users: query_rows get_all failed")
    }

    pub async fn upsert(&self, row: &UserDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("users: insert_or_update_db_entity failed");
    }
}
