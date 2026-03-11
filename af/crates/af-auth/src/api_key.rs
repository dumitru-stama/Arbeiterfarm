use crate::error::AuthError;
use af_core::identity::Identity;
use rand::Rng;
use sha2::{Digest, Sha256};
use sqlx::PgPool;

/// Generate a new API key.
///
/// Returns `(raw_key, key_hash, key_prefix)`:
/// - `raw_key`: the full key string `af_<32 alphanumeric chars>` (35 chars total)
/// - `key_hash`: SHA-256 hex digest of the raw key
/// - `key_prefix`: first 8 chars of the raw key (`af_xxxxx`)
pub fn generate_key() -> (String, String, String) {
    const CHARSET: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let mut rng = rand::thread_rng();
    let random_part: String = (0..32)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect();

    let raw_key = format!("af_{random_part}");
    let key_hash = hash_key(&raw_key);
    let key_prefix = raw_key[..8].to_string();

    (raw_key, key_hash, key_prefix)
}

/// Hash an API key with SHA-256 and return the hex digest.
fn hash_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}

/// Authenticate a raw API key against the database.
///
/// Looks up the key hash, checks user enabled status and expiry,
/// updates last_used_at, and returns a populated Identity.
pub async fn authenticate_api_key(
    pool: &PgPool,
    raw_key: &str,
) -> Result<Identity, AuthError> {
    let key_hash = hash_key(raw_key);

    let key_row = af_db::api_keys::lookup_by_hash(pool, &key_hash)
        .await?
        .ok_or(AuthError::InvalidKey)?;

    // Check expiry
    if let Some(expires_at) = key_row.expires_at {
        if expires_at < chrono::Utc::now() {
            return Err(AuthError::Expired);
        }
    }

    // Look up the owning user
    let user = af_db::users::get_by_id(pool, key_row.user_id)
        .await?
        .ok_or(AuthError::InvalidKey)?;

    if !user.enabled {
        return Err(AuthError::Disabled);
    }

    // Touch last_used_at
    af_db::api_keys::update_last_used(pool, key_row.id).await?;

    Ok(Identity {
        subject: user.subject,
        display: user.display_name,
        roles: user.roles,
        user_id: Some(user.id),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_key_format() {
        let (raw, hash, prefix) = generate_key();
        assert!(raw.starts_with("af_"), "key must start with af_");
        assert_eq!(raw.len(), 35, "key must be 35 chars (3 prefix + 32 random)");
        assert!(raw[3..].chars().all(|c| c.is_ascii_alphanumeric()),
            "random part must be alphanumeric");
        assert_eq!(hash.len(), 64, "SHA-256 hex digest must be 64 chars");
        assert_eq!(prefix.len(), 8);
        assert_eq!(&prefix, &raw[..8]);
    }

    #[test]
    fn test_generate_key_uniqueness() {
        let (k1, _, _) = generate_key();
        let (k2, _, _) = generate_key();
        assert_ne!(k1, k2, "two generated keys must differ");
    }

    #[test]
    fn test_key_hash_deterministic() {
        let input = "af_test1234567890abcdefghijklmno";
        let h1 = hash_key(input);
        let h2 = hash_key(input);
        assert_eq!(h1, h2, "same input must produce same hash");
        assert_eq!(h1.len(), 64);
    }
}
