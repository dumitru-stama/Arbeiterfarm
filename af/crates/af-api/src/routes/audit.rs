use axum::extract::{Query, State};
use axum::Json;
use std::sync::Arc;

use crate::auth::AuthenticatedUser;
use crate::dto::AuditLogResponse;
use crate::error::ApiError;
use crate::AppState;

#[derive(serde::Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    #[serde(rename = "type")]
    pub event_type: Option<String>,
}

fn default_limit() -> i64 {
    50
}

pub async fn list(
    State(state): State<Arc<AppState>>,
    AuthenticatedUser(identity): AuthenticatedUser,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<AuditLogResponse>>, ApiError> {
    let user = AuthenticatedUser(identity);
    if !user.is_admin() {
        return Err(ApiError::Forbidden("admin access required".into()));
    }

    let rows = af_db::audit_log::list(
        &state.pool,
        query.limit,
        query.event_type.as_deref(),
    )
    .await?;

    Ok(Json(rows.into_iter().map(|r| r.into()).collect()))
}
