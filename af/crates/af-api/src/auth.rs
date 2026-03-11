use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use af_auth::Action;
use af_core::Identity;
use sqlx::PgConnection;
use std::sync::Arc;
use uuid::Uuid;

use crate::error::ApiError;
use crate::AppState;

/// Extractor that authenticates via `Authorization: Bearer af_xxx` header.
pub struct AuthenticatedUser(pub Identity);

impl AuthenticatedUser {
    pub fn user_id(&self) -> Option<Uuid> {
        self.0.user_id
    }

    pub fn is_admin(&self) -> bool {
        self.0.roles.iter().any(|r| r == "admin")
    }
}

impl FromRequestParts<Arc<AppState>> for AuthenticatedUser {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ApiError::Unauthorized("missing Authorization header".into()))?;

        let token = header
            .strip_prefix("Bearer ")
            .ok_or_else(|| ApiError::Unauthorized("expected Bearer token".into()))?;

        let identity = af_auth::authenticate_api_key(&state.pool, token)
            .await
            .map_err(|e| {
                // Best-effort audit log for auth failures
                let key_prefix = if token.len() >= 8 {
                    &token[..8]
                } else {
                    token
                };
                let detail = serde_json::json!({
                    "key_prefix": key_prefix,
                    "reason": e.to_string(),
                });
                let pool = state.pool.clone();
                tokio::spawn(async move {
                    let _ = af_db::audit_log::insert(
                        &pool,
                        "auth_failure",
                        None,
                        None,
                        Some(&detail),
                    )
                    .await;
                });
                match e {
                    af_auth::AuthError::InvalidKey
                    | af_auth::AuthError::Expired
                    | af_auth::AuthError::Disabled => {
                        ApiError::Unauthorized(e.to_string())
                    }
                    af_auth::AuthError::DbError(_) => {
                        ApiError::Internal(e.to_string())
                    }
                }
            })?;

        Ok(AuthenticatedUser(identity))
    }
}

/// Check that the authenticated user has access to the given project.
/// Admin users bypass all checks.
///
/// Takes `&mut PgConnection` so it can be called inside a scoped transaction.
pub async fn require_project_access(
    db: &mut PgConnection,
    user: &AuthenticatedUser,
    project_id: Uuid,
    action: Action,
) -> Result<(), ApiError> {
    if user.is_admin() {
        return Ok(());
    }

    let user_id = user.user_id().ok_or_else(|| {
        ApiError::Forbidden("no user_id on identity".into())
    })?;

    af_auth::authz::check_project_access(db, user_id, project_id, action)
        .await
        .map_err(|e| match e {
            af_auth::AuthzError::NotMember | af_auth::AuthzError::Forbidden(_) => {
                // Uniform error message prevents project-ID enumeration
                ApiError::Forbidden("access denied".into())
            }
            af_auth::AuthzError::DbError(_) => ApiError::Internal("internal error".into()),
        })
}
