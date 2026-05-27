//! Audit ledger writes for the `denials` table (plan §7 + §10.5 #10).
//!
//! Every wallet rail rejection writes a row here so the audit trail outlives
//! Vercel's 1-day log retention. Schema lives in
//! `crates/db/migrations/0001_init.up.sql`:
//!
//! ```sql
//! CREATE TABLE denials (
//!   id            BIGSERIAL PRIMARY KEY,
//!   occurred_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
//!   code          TEXT NOT NULL,
//!   evaluator     TEXT NOT NULL,
//!   api_key_id    UUID REFERENCES api_keys(id),
//!   request_path  TEXT NOT NULL,
//!   details       JSONB
//! );
//! ```
//!
//! Anything that isn't on the rail surface (rate limit denials, API-key
//! revocations, etc.) is welcome to use this too — it's a generic audit
//! sink, not wallet-specific.

use sqlx::PgPool;
use uuid::Uuid;

/// Input row. Mirrors the `denials` columns; `id` and `occurred_at` are
/// assigned by the DB.
#[derive(Debug, Clone)]
pub struct DenialRecord {
    pub code: String,
    pub evaluator: String,
    pub api_key_id: Option<Uuid>,
    pub request_path: String,
    pub details: Option<serde_json::Value>,
}

/// Inserts a `denials` row. Returns the generated `id` on success.
pub async fn record_denial(pool: &PgPool, denial: DenialRecord) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as(
        "INSERT INTO denials (code, evaluator, api_key_id, request_path, details)
         VALUES ($1, $2, $3, $4, $5)
         RETURNING id",
    )
    .bind(&denial.code)
    .bind(&denial.evaluator)
    .bind(denial.api_key_id)
    .bind(&denial.request_path)
    .bind(&denial.details)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Returns the number of `denials` rows matching `(code, evaluator)`. Used
/// by integration tests + dashboard queries.
pub async fn count_by_code_evaluator(pool: &PgPool, code: &str, evaluator: &str) -> sqlx::Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM denials WHERE code = $1 AND evaluator = $2")
        .bind(code)
        .bind(evaluator)
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}
