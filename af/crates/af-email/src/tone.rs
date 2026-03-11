use crate::types::EmailError;
use sqlx::PgPool;

/// Validate that a tone preset name exists in the database.
/// Returns the system_instruction if valid, or EmailError if not found.
pub async fn validate_tone(pool: &PgPool, name: &str) -> Result<String, EmailError> {
    match af_db::email::get_tone_preset(pool, name).await {
        Ok(Some(preset)) => Ok(preset.system_instruction),
        Ok(None) => Err(EmailError::InvalidInput(format!(
            "unknown tone preset '{name}'. Use email.send without tone or pick from: \
             brief, formal, informal, technical, executive_summary, friendly, urgent, diplomatic"
        ))),
        Err(e) => Err(EmailError::DbError(format!("failed to load tone preset: {e}"))),
    }
}
