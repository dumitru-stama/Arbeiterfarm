use sqlx::{PgPool, Postgres, Transaction};
use uuid::Uuid;

/// Begin a transaction scoped to a specific user.
///
/// Sets `LOCAL ROLE` to `af_api` (subject to RLS) and sets the user_id
/// for RLS policy evaluation. Both settings revert when the transaction ends.
pub async fn begin_scoped(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Transaction<'_, Postgres>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("SET LOCAL ROLE af_api")
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT set_config('af.current_user_id', $1, true)")
        .bind(user_id.to_string())
        .execute(&mut *tx)
        .await?;
    Ok(tx)
}
