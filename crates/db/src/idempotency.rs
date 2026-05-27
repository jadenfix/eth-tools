//! `idempotency_keys` table — replay protection for write endpoints.
//!
//! Schema (see `migrations/0001_init.up.sql`):
//!   key_hash BYTEA PRIMARY KEY
//!   api_key_id UUID
//!   request_path TEXT
//!   response_status INT
//!   response_body JSONB
//!   created_at, expires_at TIMESTAMPTZ
//!
//! Contract: same `(api_key_id, idempotency_key, request_path)` within the TTL
//! window MUST return the cached response. Different request_path with the
//! same key = fresh execution (clients reuse keys across endpoints all the
//! time; binding to the path is what makes the spec safe).
//!
//! Storage strategy: we hash `api_key_id|path|client_key` with SHA-256 so
//! the table's PK is a fixed-size 32-byte key — client-supplied strings are
//! never stored verbatim, which makes accidental log leakage a non-issue.

use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

/// Cached prior response. Returned verbatim to the client on hit.
#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub response_status: i32,
    pub response_body: Value,
}

/// SHA-256(api_key_id || ":" || path || ":" || client_key). Stable hash that
/// makes the cache PK fixed-size and means client-supplied strings never sit
/// in the DB in plaintext.
pub fn hash_key(api_key_id: Option<Uuid>, path: &str, client_key: &str) -> Vec<u8> {
    let mut h = Sha256::new();
    match api_key_id {
        Some(id) => h.update(id.as_bytes()),
        // Anonymous-but-test callers (`et_test_dev`) get a fixed namespace so
        // local + integration tests are deterministic.
        None => h.update([0u8; 16]),
    }
    h.update(b":");
    h.update(path.as_bytes());
    h.update(b":");
    h.update(client_key.as_bytes());
    h.finalize().to_vec()
}

/// Lookup an existing cached response. Returns `None` if no live row exists
/// (expired rows are filtered out — caller should treat them as misses and
/// re-execute the handler).
pub async fn lookup(pool: &PgPool, key_hash: &[u8]) -> Result<Option<CachedResponse>, sqlx::Error> {
    let row: Option<(i32, Value)> = sqlx::query_as(
        "SELECT response_status, response_body
         FROM idempotency_keys
         WHERE key_hash = $1 AND expires_at > NOW()",
    )
    .bind(key_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(status, body)| CachedResponse { response_status: status, response_body: body }))
}

/// Persist a (status, body) tuple under `key_hash` for `ttl_seconds`. Caller
/// computes `key_hash` via `hash_key()`. Uses `ON CONFLICT DO NOTHING` so two
/// near-simultaneous requests with the same key both succeed on the API side
/// without one getting a 23505 unique-violation 500.
pub async fn store(
    pool: &PgPool,
    key_hash: &[u8],
    api_key_id: Option<Uuid>,
    request_path: &str,
    response_status: i32,
    response_body: &Value,
    ttl_seconds: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO idempotency_keys
            (key_hash, api_key_id, request_path, response_status, response_body,
             created_at, expires_at)
         VALUES
            ($1, $2, $3, $4, $5, NOW(), NOW() + ($6 || ' seconds')::INTERVAL)
         ON CONFLICT (key_hash) DO NOTHING",
    )
    .bind(key_hash)
    .bind(api_key_id)
    .bind(request_path)
    .bind(response_status)
    .bind(response_body)
    .bind(ttl_seconds.to_string())
    .execute(pool)
    .await?;
    Ok(())
}
