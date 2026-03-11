use chrono::{DateTime, Utc};
use sqlx::{FromRow, PgPool, Postgres};

#[derive(Debug, Clone, FromRow)]
pub struct BlobRow {
    pub sha256: String,
    pub size_bytes: i64,
    pub storage_path: String,
    pub created_at: DateTime<Utc>,
}

/// Upsert a blob record. Multi-query (INSERT then fallback SELECT), requires &PgPool.
pub async fn upsert_blob(
    pool: &PgPool,
    sha256: &str,
    size_bytes: i64,
    storage_path: &str,
) -> Result<BlobRow, sqlx::Error> {
    let row = sqlx::query_as::<_, BlobRow>(
        "INSERT INTO blobs (sha256, size_bytes, storage_path) VALUES ($1, $2, $3)
         ON CONFLICT (sha256) DO NOTHING
         RETURNING sha256, size_bytes, storage_path, created_at",
    )
    .bind(sha256)
    .bind(size_bytes)
    .bind(storage_path)
    .fetch_optional(pool)
    .await?;

    // If ON CONFLICT DO NOTHING fired, fetch the existing row
    match row {
        Some(r) => Ok(r),
        None => {
            sqlx::query_as::<_, BlobRow>(
                "SELECT sha256, size_bytes, storage_path, created_at FROM blobs WHERE sha256 = $1",
            )
            .bind(sha256)
            .fetch_one(pool)
            .await
        }
    }
}

pub async fn get_blob(
    db: impl sqlx::Executor<'_, Database = Postgres>,
    sha256: &str,
) -> Result<Option<BlobRow>, sqlx::Error> {
    sqlx::query_as::<_, BlobRow>(
        "SELECT sha256, size_bytes, storage_path, created_at FROM blobs WHERE sha256 = $1",
    )
    .bind(sha256)
    .fetch_optional(db)
    .await
}

/// Identify blobs not referenced by any artifact (candidate scan only, no mutation).
pub async fn find_unreferenced_blobs(pool: &PgPool) -> Result<Vec<(String, String)>, sqlx::Error> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT b.sha256, b.storage_path FROM blobs b \
         WHERE NOT EXISTS (SELECT 1 FROM artifacts a WHERE a.sha256 = b.sha256)",
    )
    .fetch_all(pool)
    .await
}

/// Atomically delete a blob row only if it is still unreferenced.
/// Returns true if the row was deleted (still unreferenced), false if it was
/// re-referenced between the candidate scan and this call (race avoided).
pub async fn delete_blob_if_unreferenced(
    pool: &PgPool,
    sha256: &str,
) -> Result<bool, sqlx::Error> {
    let result = sqlx::query(
        "DELETE FROM blobs WHERE sha256 = $1 \
         AND NOT EXISTS (SELECT 1 FROM artifacts a WHERE a.sha256 = $1)",
    )
    .bind(sha256)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
}
