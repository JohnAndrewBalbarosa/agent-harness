//! `GET /health`: proves the database answers before reporting ok.

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use super::SharedDatabase;
use crate::http_errors::HubError;

const SCHEMA_VERSION: i64 = 1;

pub async fn report_health(
    State(database): State<SharedDatabase>,
) -> Result<Json<Value>, HubError> {
    database
        .read(|connection| Ok(connection.query_row("SELECT 1", [], |row| row.get::<_, i64>(0))?))
        .await?;
    Ok(Json(
        json!({ "status": "ok", "schemaVersion": SCHEMA_VERSION }),
    ))
}
