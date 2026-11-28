//! Creates the tables of `infra/tables.json`, for DynamoDB Local.

use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, GlobalSecondaryIndex, KeySchemaElement, KeyType, Projection,
    ProjectionType, ScalarAttributeType, StreamSpecification, StreamViewType,
    TimeToLiveSpecification,
};
use serde_json::Value;

use crate::db::{Db, DbError, Tables, unavailable};

/// `infra/tables.json`.
pub const TABLES_JSON: &str = include_str!("../../../infra/tables.json");

fn invalid(message: &str) -> DbError {
    DbError::Invalid(format!("infra/tables.json: {message}"))
}

fn key_schema(value: &Value) -> Result<Vec<KeySchemaElement>, DbError> {
    value
        .as_array()
        .ok_or_else(|| invalid("a key schema is a list"))?
        .iter()
        .map(|k| {
            let key_type = match k["KeyType"].as_str() {
                Some("HASH") => KeyType::Hash,
                Some("RANGE") => KeyType::Range,
                _ => return Err(invalid("a key type is HASH or RANGE")),
            };
            KeySchemaElement::builder()
                .attribute_name(k["AttributeName"].as_str().unwrap_or_default())
                .key_type(key_type)
                .build()
                .map_err(|e| invalid(&e.to_string()))
        })
        .collect()
}

/// Creates the `main` and `audit` tables with the keys, indexes, stream and TTL of
/// `infra/tables.json`, under the names of `tables`.
pub async fn create_tables(db: &Db) -> Result<(), DbError> {
    let root: Value = serde_json::from_str(TABLES_JSON).map_err(|e| invalid(&e.to_string()))?;
    let tables: &Tables = db.tables();
    for table in root["tables"]
        .as_array()
        .ok_or_else(|| invalid("no tables"))?
    {
        let name = match table["role"].as_str() {
            Some("main") => &tables.main,
            Some("audit") => &tables.audit,
            _ => return Err(invalid("a role is main or audit")),
        };
        let p = &table["properties"];
        let mut create = db
            .client()
            .create_table()
            .table_name(name)
            .billing_mode(BillingMode::PayPerRequest)
            .set_key_schema(Some(key_schema(&p["KeySchema"])?));
        for a in p["AttributeDefinitions"].as_array().into_iter().flatten() {
            create = create.attribute_definitions(
                AttributeDefinition::builder()
                    .attribute_name(a["AttributeName"].as_str().unwrap_or_default())
                    .attribute_type(ScalarAttributeType::S)
                    .build()
                    .map_err(|e| invalid(&e.to_string()))?,
            );
        }
        for i in p["GlobalSecondaryIndexes"].as_array().into_iter().flatten() {
            create = create.global_secondary_indexes(
                GlobalSecondaryIndex::builder()
                    .index_name(i["IndexName"].as_str().unwrap_or_default())
                    .set_key_schema(Some(key_schema(&i["KeySchema"])?))
                    .projection(
                        Projection::builder()
                            .projection_type(ProjectionType::All)
                            .build(),
                    )
                    .build()
                    .map_err(|e| invalid(&e.to_string()))?,
            );
        }
        if p["StreamSpecification"]["StreamViewType"] == "NEW_IMAGE" {
            create = create.stream_specification(
                StreamSpecification::builder()
                    .stream_enabled(true)
                    .stream_view_type(StreamViewType::NewImage)
                    .build()
                    .map_err(|e| invalid(&e.to_string()))?,
            );
        }
        create.send().await.map_err(unavailable)?;
        if let Some(attribute) = p["TimeToLiveSpecification"]["AttributeName"].as_str() {
            db.client()
                .update_time_to_live()
                .table_name(name)
                .time_to_live_specification(
                    TimeToLiveSpecification::builder()
                        .attribute_name(attribute)
                        .enabled(true)
                        .build()
                        .map_err(|e| invalid(&e.to_string()))?,
                )
                .send()
                .await
                .map_err(unavailable)?;
        }
    }
    Ok(())
}
