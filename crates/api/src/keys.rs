//! API key issuance + revocation (Phase-5).
//!
//! Plaintext format: `et_live_<24 base62>` in production,
//! `et_test_<24 base62>` everywhere else. Total length 32; `key_prefix` is the
//! first 11 chars (`et_live_<3 base62>`) — 3 chars of random give ~238k unique
//! prefixes which is plenty for indexed lookup (collisions are handled by the
//! UNIQUE constraint + bcrypt verify).
//!
//! The plaintext is returned to the caller **once** and never logged or
//! stored. Only the bcrypt(cost=10) hash and the prefix persist.

use eth_tools_db::api_keys::KeyRow;
use rand::distributions::{Distribution, Uniform};
use rand::rngs::OsRng;
use sqlx::PgPool;
use uuid::Uuid;

const BASE62: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const RANDOM_LEN: usize = 24;
/// First 11 chars of the plaintext: 8 chars of `et_live_` / `et_test_` plus 3
/// chars of randomness. Indexed unique in Postgres so prefix lookup is O(log n).
pub const PREFIX_LEN: usize = 11;
/// bcrypt cost. 10 ≈ 100ms on modern hardware — slow enough to make brute
/// force impractical, fast enough that we can cache verifies for 60s and not
/// melt the API on issuance.
pub const BCRYPT_COST: u32 = 10;

/// Returned by [`issue`]. The plaintext is the only thing the user will ever
/// see; the caller must surface it to them immediately and never log it.
#[derive(Debug)]
pub struct IssuedKey {
    pub id: Uuid,
    pub plaintext: String,
    pub prefix: String,
}

#[derive(Debug, thiserror::Error)]
pub enum KeyError {
    #[error("invalid scope: {0}")]
    InvalidScope(String),
    #[error("invalid name")]
    InvalidName,
    #[error("bcrypt: {0}")]
    Bcrypt(#[from] bcrypt::BcryptError),
    #[error("db: {0}")]
    Db(#[from] sqlx::Error),
    #[error("task join: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Allowed scopes. Keep this tight; widening here changes the auth surface.
pub const ALLOWED_SCOPES: &[&str] = &["read", "write", "wallet"];

/// True if we should prefix with `et_live_` (prod) versus `et_test_`
/// (preview/dev). Vercel injects `VERCEL_ENV` at deploy time; absence (local
/// dev) is treated as test.
pub fn is_live_env() -> bool {
    std::env::var("VERCEL_ENV")
        .map(|v| v == "production")
        .unwrap_or(false)
}

/// Generate a random plaintext key. Uses `OsRng` (cryptographic) — `thread_rng`
/// would also work but `OsRng` makes the intent explicit. 24 base62 chars
/// ≈ 142 bits of entropy.
fn generate_plaintext() -> String {
    let prefix_str = if is_live_env() { "et_live_" } else { "et_test_" };
    let dist = Uniform::from(0..BASE62.len());
    let mut rng = OsRng;
    let mut out = String::with_capacity(prefix_str.len() + RANDOM_LEN);
    out.push_str(prefix_str);
    for _ in 0..RANDOM_LEN {
        out.push(BASE62[dist.sample(&mut rng)] as char);
    }
    out
}

/// Validate user-supplied scopes against the allowlist. Empty list defaults to
/// `["read"]` at the caller; this fn only rejects unknown values.
pub fn validate_scopes(scopes: &[String]) -> Result<(), KeyError> {
    for s in scopes {
        if !ALLOWED_SCOPES.contains(&s.as_str()) {
            return Err(KeyError::InvalidScope(s.clone()));
        }
    }
    Ok(())
}

/// Mint a new API key for `(github_user_id, github_login)`.
///
/// Steps:
///   1. CSPRNG-generate plaintext.
///   2. bcrypt-hash on a blocking thread (cost=10 is ~100ms; never do this on
///      the tokio runtime — it would starve other requests).
///   3. Insert `(id, prefix, hash, name, scopes, owner)` into `api_keys`.
///   4. Return plaintext + id + prefix.
///
/// The plaintext is the **only** way to recover the bearer; the caller must
/// surface it to the user once and discard it.
pub async fn issue(
    github_user_id: i64,
    github_login: &str,
    name: &str,
    scopes: &[String],
    pool: &PgPool,
) -> Result<IssuedKey, KeyError> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 128 {
        return Err(KeyError::InvalidName);
    }
    validate_scopes(scopes)?;
    let scopes = if scopes.is_empty() {
        vec!["read".to_string()]
    } else {
        scopes.to_vec()
    };

    let plaintext = generate_plaintext();
    let prefix = plaintext[..PREFIX_LEN].to_string();
    let id = Uuid::new_v4();

    // bcrypt::hash holds the CPU for ~100ms at cost=10. Run on a blocking
    // thread so the async runtime can keep serving other requests.
    let plaintext_for_hash = plaintext.clone();
    let hash = tokio::task::spawn_blocking(move || {
        bcrypt::hash(plaintext_for_hash, BCRYPT_COST)
    })
    .await??;

    eth_tools_db::api_keys::insert(
        pool,
        eth_tools_db::api_keys::NewKey {
            id,
            github_user_id,
            github_login,
            key_prefix: &prefix,
            key_hash: &hash,
            name: trimmed,
            scopes: &scopes,
        },
    )
    .await?;

    // Deliberately do not `tracing::info!` the plaintext or hash. Log only
    // the non-sensitive identifiers.
    tracing::info!(
        key_id = %id,
        prefix = %prefix,
        github_login = %github_login,
        scopes = ?scopes,
        "api_key issued",
    );

    Ok(IssuedKey {
        id,
        plaintext,
        prefix,
    })
}

/// Revoke a key. Owner-checked in SQL. Returns `Ok(false)` when no row
/// matched (already revoked or wrong owner); the handler maps that to 404.
pub async fn revoke(id: Uuid, github_user_id: i64, pool: &PgPool) -> Result<bool, KeyError> {
    let n = eth_tools_db::api_keys::revoke(pool, id, github_user_id).await?;
    Ok(n > 0)
}

/// List a user's non-revoked keys. Hashes are never returned.
pub async fn list_for_user(
    github_user_id: i64,
    pool: &PgPool,
) -> Result<Vec<KeyRow>, KeyError> {
    Ok(eth_tools_db::api_keys::list_for_user(pool, github_user_id).await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plaintext_shape_and_prefix() {
        // The two env branches print different scheme tags but always the
        // same total length (8 + 24 = 32) and prefix length (11).
        std::env::remove_var("VERCEL_ENV");
        let key = generate_plaintext();
        assert_eq!(key.len(), 32);
        assert!(key.starts_with("et_test_"));
        let prefix = &key[..PREFIX_LEN];
        assert_eq!(prefix.len(), PREFIX_LEN);
        assert!(prefix.starts_with("et_test_"));

        std::env::set_var("VERCEL_ENV", "production");
        let key = generate_plaintext();
        assert!(key.starts_with("et_live_"));
        std::env::remove_var("VERCEL_ENV");
    }

    #[test]
    fn plaintext_is_base62_after_prefix() {
        let key = generate_plaintext();
        let body = &key[8..];
        for b in body.bytes() {
            assert!(BASE62.contains(&b), "non-base62 byte 0x{b:02x} in {key}");
        }
    }

    #[test]
    fn bcrypt_round_trip() {
        // Use cost=4 for tests so it's fast. Verifying against a cost=10 hash
        // would still work but slows the test suite for no benefit.
        let plaintext = generate_plaintext();
        let hash = bcrypt::hash(&plaintext, 4).unwrap();
        assert!(bcrypt::verify(&plaintext, &hash).unwrap());
        assert!(!bcrypt::verify("wrong-key", &hash).unwrap());
    }

    #[test]
    fn unknown_scope_rejected() {
        let scopes = vec!["read".to_string(), "admin".to_string()];
        match validate_scopes(&scopes) {
            Err(KeyError::InvalidScope(s)) => assert_eq!(s, "admin"),
            other => panic!("expected InvalidScope, got {other:?}"),
        }
    }

    #[test]
    fn allowed_scopes_accepted() {
        for s in ALLOWED_SCOPES {
            let scopes = vec![s.to_string()];
            assert!(validate_scopes(&scopes).is_ok(), "should accept {s}");
        }
    }
}
