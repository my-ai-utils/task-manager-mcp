use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use service_sdk::my_postgres::sql_where::NoneWhereModel;
use service_sdk::my_postgres::{MyPostgres, UpdateConflictType};

service_sdk::macros::use_my_postgres!();

use crate::app::APP_NAME;
use crate::settings::SettingsReader;

pub const TABLE_NAME: &str = "column_templates";
pub const PK_NAME: &str = "column_templates_pk";

// One column inside the template row's `columns` jsonb.
//
// `column_order` rather than `order`: the schema generator does not quote identifiers and `order` is a
// reserved word. This is a JSON key rather than a column, but keeping both spellings identical avoids a
// trap the next person would have to rediscover.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct ColumnTemplateColumnJsonModel {
    pub id: String,
    pub name: String,
    pub description: String,
    pub column_order: i32,
}

// A named set of columns, shared by any number of projects.
//
// Held whole in one row: the columns are only ever read and written as a set, and a project points at
// the template rather than at individual columns, so there is nothing to join.
#[derive(SelectDbEntity, InsertDbEntity, UpdateDbEntity, TableSchema, Debug)]
pub struct ColumnTemplateDto {
    #[primary_key(0)]
    pub id: String,
    pub name: String,
    pub description: String,
    #[sql_type("jsonb")]
    #[json]
    pub columns: Vec<ColumnTemplateColumnJsonModel>,
    #[sql_type("timestamp")]
    pub created: DateTimeAsMicroseconds,
}

#[derive(WhereDbModel, Debug)]
pub struct ByIdWhereModel<'s> {
    pub id: &'s str,
}

pub struct ColumnTemplatesRepo {
    postgres: MyPostgres,
}

impl ColumnTemplatesRepo {
    pub async fn new(settings_reader: Arc<SettingsReader>) -> Self {
        let postgres = MyPostgres::from_settings(APP_NAME, settings_reader)
            .with_table_schema_verification::<ColumnTemplateDto>(TABLE_NAME, Some(PK_NAME.into()))
            .build()
            .await;

        Self { postgres }
    }

    /// Every template. Called once at startup — after that the answer lives in memory.
    pub async fn get_all(&self, ctx: &MyTelemetryContext) -> Vec<ColumnTemplateDto> {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .query_rows(TABLE_NAME, NoneWhereModel::new(), Some(ctx))
            .await
            .expect("column_templates: query_rows get_all failed")
    }

    pub async fn upsert(&self, row: &ColumnTemplateDto, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .insert_or_update_db_entity(
                TABLE_NAME,
                UpdateConflictType::OnPrimaryKeyConstraint(PK_NAME.into()),
                row,
                Some(ctx),
            )
            .await
            .expect("column_templates: insert_or_update_db_entity failed");
    }

    pub async fn delete(&self, id: &str, ctx: &MyTelemetryContext) {
        self.postgres
            .with_retries(3, Duration::from_secs(1))
            .delete(TABLE_NAME, &ByIdWhereModel { id }, Some(ctx))
            .await
            .expect("column_templates: delete failed");
    }
}
