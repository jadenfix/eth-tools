//! Typed access to the `cursors` table (plan §3 invariant 5).
//!
//! Each row records the last `(block_number, log_index)` a worker processed
//! for a specific `(chain_id, contract, topic0)` log subscription. Workers
//! resume from `last_block + 1` (or the same block, filtered by
//! `log_index > last_log_index`) on the next tick.
//!
//! ## Why `&mut PgConnection`, not `&PgPool`?
//!
//! Plan §3 invariant 5: "Cursor advance: advanced in the same transaction as
//! the upsert. Restart-safe." A worker that took a `&PgPool` here would have
//! to open a *second* connection just to bump the cursor — defeating the
//! transactional guarantee. Taking `&mut PgConnection` lets the worker call
//! `pool.begin()` once, fold both its event-row INSERT and `advance()` into
//! the same Tx, and `commit()`.
//!
//! Composition example (real shape lands in PR3):
//! ```ignore
//! let mut tx = pool.begin().await?;
//! for log in &batch {
//!     sqlx::query("INSERT INTO agents …").execute(&mut *tx).await?;
//! }
//! cursors::advance(&mut *tx, &key, last_block, last_log_index).await?;
//! tx.commit().await?;
//! ```

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, PgConnection};

/// One row of the `cursors` table.
#[derive(Debug, Clone, FromRow, Serialize, Deserialize, PartialEq, Eq)]
pub struct Cursor {
    pub cursor_key: String,
    pub last_block: i64,
    pub last_log_index: i32,
    pub updated_at: DateTime<Utc>,
}

/// Fetch the cursor for `cursor_key`, or `None` if the worker has never run
/// against this `(chain, contract, topic)` triple before.
pub async fn get(
    executor: &mut PgConnection,
    cursor_key: &str,
) -> sqlx::Result<Option<Cursor>> {
    sqlx::query_as::<_, Cursor>(
        "SELECT cursor_key, last_block, last_log_index, updated_at
         FROM cursors WHERE cursor_key = $1",
    )
    .bind(cursor_key)
    .fetch_optional(executor)
    .await
}

/// Upsert the cursor to `(last_block, last_log_index)` and stamp
/// `updated_at = NOW()`. Restart-safe: a worker that crashes after advancing
/// re-reads the same cursor on next tick and skips the already-processed
/// prefix via `WHERE block_number > last_block OR (block_number = last_block
/// AND log_index > last_log_index)` in its event scan.
///
/// SAFETY: caller must invoke this from within the same transaction as the
/// event-row writes it gates. See module docs.
pub async fn advance(
    executor: &mut PgConnection,
    cursor_key: &str,
    last_block: i64,
    last_log_index: i32,
) -> sqlx::Result<()> {
    sqlx::query(
        "INSERT INTO cursors (cursor_key, last_block, last_log_index, updated_at)
         VALUES ($1, $2, $3, NOW())
         ON CONFLICT (cursor_key) DO UPDATE
           SET last_block     = EXCLUDED.last_block,
               last_log_index = EXCLUDED.last_log_index,
               updated_at     = NOW()",
    )
    .bind(cursor_key)
    .bind(last_block)
    .bind(last_log_index)
    .execute(executor)
    .await?;
    Ok(())
}

/// Canonical cursor key shape: `"{chain_id}:{0xhex_contract}:{0xhex_topic}"`.
/// Lowercase hex throughout so two workers never produce drifting keys for
/// the same subscription (a `0xAB…` vs `0xab…` mismatch would silently
/// double-scan the same range).
///
/// 20-byte contract → 42 chars (`0x` + 40 hex); 32-byte topic → 66 chars.
pub fn cursor_key(chain_id: i64, contract: &[u8; 20], topic0: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    // Pre-size: "{i64}:{42}:{66}" worst-case ≈ 130 bytes — well under 4 KiB
    // small-string heuristic but still worth pre-allocating.
    let mut s = String::with_capacity(20 + 1 + 42 + 1 + 66);
    let _ = write!(s, "{chain_id}:0x");
    for b in contract {
        let _ = write!(s, "{b:02x}");
    }
    s.push_str(":0x");
    for b in topic0 {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_key_is_lowercase_and_deterministic() {
        let contract = [0xABu8; 20];
        let topic = [0xCDu8; 32];
        let k = cursor_key(8453, &contract, &topic);
        // Lowercase hex, no uppercase chars anywhere.
        assert!(!k.chars().any(|c| c.is_ascii_uppercase()));
        // Exact shape.
        let expected_contract = "ab".repeat(20);
        let expected_topic = "cd".repeat(32);
        assert_eq!(k, format!("8453:0x{expected_contract}:0x{expected_topic}"));
        // Same inputs → same key (no hidden state).
        assert_eq!(k, cursor_key(8453, &contract, &topic));
    }

    #[test]
    fn cursor_key_distinguishes_chains_and_contracts() {
        let c = [0u8; 20];
        let t = [0u8; 32];
        assert_ne!(cursor_key(1, &c, &t), cursor_key(8453, &c, &t));
        let c2 = [1u8; 20];
        assert_ne!(cursor_key(1, &c, &t), cursor_key(1, &c2, &t));
    }
}
