use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "kind_templates";
pub const PK_NAME: &str = "kind_templates_pk";

// One task type inside the template row's `kinds` jsonb. `color` is a
// `task_manager_shared::kind_color::KindColor` wire value; an unrecognised one reads as the default
// swatch rather than failing the load.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct KindTemplateKindJsonModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub color: String,
    // An icon name without extension, or empty. `default` so a row written before icons existed still
    // deserialises.
    #[serde(default)]
    pub icon: String,
}

// A named set of task types, shared by any number of projects.
//
// Held whole in one row: the types are only ever read and written as a set, and a project points at the
// template rather than at individual types, so there is nothing to join.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct KindTemplateDto {
    #[primary_key(0)]
    pub id: String,
    pub name: String,
    pub description: String,
    #[sql_type("jsonb")]
    #[json]
    pub kinds: Vec<KindTemplateKindJsonModel>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
}

#[derive(WhereDbModel, Debug)]
pub struct KindTemplateByIdWhereModel<'s> {
    pub id: &'s str,
}

pub struct KindTemplatesRepo {
    postgres: MyPostgres,
}

impl KindTemplatesRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<KindTemplateDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every template. Called once at startup — after that the answer lives in memory.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<KindTemplateDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("kind_templates: query_rows get_all failed")
    }

    pub async fn upsert(&self, row: &KindTemplateDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("kind_templates: insert_or_update_db_entity failed");
    }

    pub async fn delete(&self, id: &str, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .delete(TABLE_NAME, &KindTemplateByIdWhereModel { id }, Some(ctx))
            .await
            .expect("kind_templates: delete failed");
    }
}
