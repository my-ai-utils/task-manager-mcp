use std::sync::Arc;
use std::time::Duration;

use service_sdk::my_postgres::MyPostgres;
use service_sdk::my_postgres::sql_where::NoneWhereModel;

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "project_members";
pub const PK_NAME: &str = "project_members_pk";

// Who may see a project. Its own table rather than a jsonb list on the project row because the set is
// edited from the project's side but read from the user's ("which boards may this person see?"), and
// a row per pair answers both directions without scanning.
//
// No foreign key to `users`: a membership naming an address with no user row is possible (an admin
// pasting an email before the person is created) and must read as "not a member yet" rather than as a
// write that is refused.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct ProjectMemberDto {
    #[primary_key(0)]
    pub project_id: String,
    #[primary_key(1)]
    pub email: String,
}

// EVERY column here is part of the primary key, and that rules out an upsert: the generator emits
// `ON CONFLICT ... DO UPDATE SET <the non-key columns>`, there are none, and Postgres refuses
// `DO UPDATE SET` with an empty list — "syntax error at end of input". Which is fine, because there is
// genuinely nothing to update on a pair that is already there. Insert-if-not-exists says exactly that.

// Membership is replaced wholesale, so the delete side only ever needs the project.
#[derive(WhereDbModel, Debug)]
pub struct DeleteProjectMembersWhereModel<'s> {
    pub project_id: &'s str,
}

pub struct ProjectMembersRepo {
    postgres: MyPostgres,
}

impl ProjectMembersRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<ProjectMemberDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every membership pair. Called once at startup.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<ProjectMemberDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("project_members: query_rows get_all failed")
    }

    /// Replace a project's whole membership: drop what is there, insert what was asked for.
    ///
    /// Not a transaction — my-postgres exposes none — so the window between the two writes is real. It
    /// is survivable because the caller applies the same change to memory only after both succeed,
    /// and because a crash inside the window leaves the project with fewer members than intended
    /// rather than with a member who should not be there.
    pub async fn replace_for_project(
        &self,
        project_id: &str,
        emails: &[String],
        ctx: &MyTelemetryContext,
    ) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .delete(
                TABLE_NAME,
                &DeleteProjectMembersWhereModel { project_id },
                Some(ctx),
            )
            .await
            .expect("project_members: delete failed");

        if emails.is_empty() {
            return;
        }

        let rows: Vec<ProjectMemberDto> = emails
            .iter()
            .map(|email| ProjectMemberDto {
                project_id: project_id.to_string(),
                email: email.clone(),
            })
            .collect();

        // Insert-if-not-exists rather than an upsert. See the note on the DTO: with every column in the
        // primary key there is nothing to update, and asking for an update produced invalid SQL.
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .bulk_insert_db_entities_if_not_exists(TABLE_NAME, &rows, Some(ctx))
            .await
            .expect("project_members: bulk_insert_db_entities_if_not_exists failed");
    }
}
