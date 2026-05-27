//! Typed queries against the `api_keys` table (Phase-5).
//!
//! Plaintext keys NEVER touch this module. Callers in `crates/api::keys` are
//! responsible for generating the plaintext, computing the bcrypt hash, and
//! passing both the prefix and the hash here. We store only the prefix (for
//! lookup) and the hash (for constant-time verify).
//!
//! Schema lives in `migrations/0001_init.up.sql`:
//!   id UUID PK, github_user_id BIGINT, github_login TEXT, key_prefix TEXT
//!   UNIQUE, key_hash TEXT, name TEXT, scopes TEXT[], created_at TIMESTAMPTZ,
//!   last_used_at TIMESTAMPTZ, revoked_at TIMESTAMPTZ.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgPool};
use uuid::Uuid;

/// One row of the `api_keys` table, hash stripped for safe-to-return shapes.
/// Use this for `list_for_user` / dashboard responses.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize)]
pub struct KeyRow {
    pub id: Uuid,
    pub github_user_id: i64,
    pub github_login: String,
    pub key_prefix: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub created_at: DateTime<Utc>,
    pub last_used_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

/// Shape returned by `lookup_by_prefix` — includes the bcrypt hash so the auth
/// middleware can verify a candidate bearer token. NEVER serialized to a
/// client.
#[derive(Debug, Clone, FromRow)]
pub struct VerifyRow {
    pub id: Uuid,
    pub github_user_id: i64,
    pub github_login: String,
    pub key_hash: String,
    pub scopes: Vec<String>,
}

/// Insert payload bundled into one struct so the `insert` call site doesn't
/// trip clippy's `too_many_arguments` lint (and so callers can't accidentally
/// swap two string args of the same type).
#[derive(Debug)]
pub struct NewKey<'a> {
    pub id: Uuid,
    pub github_user_id: i64,
    pub github_login: &'a str,
    pub key_prefix: &'a str,
    pub key_hash: &'a str,
    pub name: &'a str,
    pub scopes: &'a [String],
}

/// Insert a freshly-issued key. Caller has already generated the plaintext +
/// hash; we store the hash only.
pub async fn insert(pool: &PgPool, k: NewKey<'_>) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO api_keys (id, github_user_id, github_login, key_prefix, key_hash, name, scopes) \
         VALUES ($1, $2, $3, $4, $5, $6, $7)",
    )
    .bind(k.id)
    .bind(k.github_user_id)
    .bind(k.github_login)
    .bind(k.key_prefix)
    .bind(k.key_hash)
    .bind(k.name)
    .bind(k.scopes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Look up a non-revoked key by prefix. Used by the bearer middleware before
/// the bcrypt compare. Returns `None` if no row matches OR the row is revoked.
pub async fn lookup_by_prefix(
    pool: &PgPool,
    key_prefix: &str,
) -> Result<Option<VerifyRow>, sqlx::Error> {
    sqlx::query_as::<_, VerifyRow>(
        "SELECT id, github_user_id, github_login, key_hash, scopes \
         FROM api_keys \
         WHERE key_prefix = $1 AND revoked_at IS NULL",
    )
    .bind(key_prefix)
    .fetch_optional(pool)
    .await
}

/// Mark a key revoked. Owner check is enforced in SQL — we filter on
/// `github_user_id` so an attacker with another user's session can't revoke
/// keys they don't own. Returns the row count so the caller can distinguish
/// "not found" from "not yours" if it cares (we currently coalesce both into a
/// single 404).
pub async fn revoke(pool: &PgPool, id: Uuid, github_user_id: i64) -> Result<u64, sqlx::Error> {
    let result = sqlx::query(
        "UPDATE api_keys SET revoked_at = NOW() \
         WHERE id = $1 AND github_user_id = $2 AND revoked_at IS NULL",
    )
    .bind(id)
    .bind(github_user_id)
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}

/// All non-revoked keys for a user, ordered newest first.
///
/// Returns hashes-stripped rows; safe to serialize.
pub async fn list_for_user(
    pool: &PgPool,
    github_user_id: i64,
) -> Result<Vec<KeyRow>, sqlx::Error> {
    sqlx::query_as::<_, KeyRow>(
        "SELECT id, github_user_id, github_login, key_prefix, name, scopes, \
                created_at, last_used_at, revoked_at \
         FROM api_keys \
         WHERE github_user_id = $1 AND revoked_at IS NULL \
         ORDER BY created_at DESC",
    )
    .bind(github_user_id)
    .fetch_all(pool)
    .await
}

/// Best-effort `last_used_at` bump. Callers should `tokio::spawn` this — a
/// failure here must not block the request.
pub async fn touch_last_used(pool: &PgPool, id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE api_keys SET last_used_at = NOW() WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
