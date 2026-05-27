//! Typed access to the `worker_runs` audit table (plan §3 invariant 7).
//!
//! Every cron worker writes exactly one row per invocation: a `begin()` at
//! function entry (rolls a row with `finished_at IS NULL, ok IS NULL`), and
//! a terminal `finish_ok` / `finish_err` before returning. The
//! `/dashboard/workers` page polls this table for liveness; the wallet-
//! balance-keeper watchdog (plan §3 W8) scans it for stale workers.
//!
//! ## Why an opaque `RunHandle` instead of returning `i64`?
//!
//! Two reasons:
//!   1. Callers can't accidentally pass the run id to `finish_*` of a
//!      different run. The handle is bound to one `begin()` site.
//!   2. We can grow the handle later (e.g. attach a Tokio span, a
//!      `Instant` for elapsed-time logging) without breaking call sites.
//!
//! ## Error truncation
//!
//! `finish_err(..., error)` truncates `error` to 8 KiB before INSERT. A
//! pathological worker that hands us a multi-megabyte panic message
//! shouldn't be allowed to bloat the audit row past anything the dashboard
//! can render — and Postgres' TEXT is technically unbounded but expensive
//! to read in bulk. 8 KiB matches Vercel's per-log-line cap.

use chrono::{DateTime, Utc};
use sqlx::PgPool;

/// Opaque token returned from [`begin`]; pass to [`finish_ok`] or
/// [`finish_err`] to terminate the run. Cannot be constructed from outside
/// this module — guarantees every finish corresponds to a real begin.
#[derive(Debug)]
pub struct RunHandle {
    run_id: i64,
    #[allow(dead_code)] // exposed via getter; useful for log spans in PR3.
    started_at: DateTime<Utc>,
}

impl RunHandle {
    /// Surface the underlying id for log correlation (the `worker_runs.id`
    /// column shows up on `/dashboard/workers`).
    pub fn run_id(&self) -> i64 {
        self.run_id
    }

    /// Stamp recorded at `begin()` for elapsed-time computation.
    pub fn started_at(&self) -> DateTime<Utc> {
        self.started_at
    }
}

/// Error-message hard cap. Anything longer is truncated with an explicit
/// `…(truncated)` marker so dashboard readers know they're seeing a prefix.
const ERROR_MAX_BYTES: usize = 8 * 1024;

/// Insert a fresh `worker_runs` row with `finished_at = NULL, ok = NULL`.
/// `dryrun` writes are NOT audited (plan §3 invariant 4: dry-run "runs all
/// reads but skips writes"); callers should branch before calling `begin`.
pub async fn begin(
    pool: &PgPool,
    worker_name: &str,
    vercel_env: &str,
    dryrun: bool,
) -> sqlx::Result<RunHandle> {
    let (run_id, started_at): (i64, DateTime<Utc>) = sqlx::query_as(
        "INSERT INTO worker_runs (worker_name, vercel_env, dryrun)
         VALUES ($1, $2, $3)
         RETURNING id, started_at",
    )
    .bind(worker_name)
    .bind(vercel_env)
    .bind(dryrun)
    .fetch_one(pool)
    .await?;
    Ok(RunHandle { run_id, started_at })
}

/// Mark the run successful and record row counts. `rows_in` is the count of
/// events / endpoints / agents the worker read; `rows_out` is what it wrote
/// (or would have, under dry-run). The dashboard surfaces both for SLO
/// visibility.
pub async fn finish_ok(
    pool: &PgPool,
    handle: RunHandle,
    rows_in: i32,
    rows_out: i32,
) -> sqlx::Result<()> {
    sqlx::query(
        "UPDATE worker_runs
            SET finished_at = NOW(),
                ok          = TRUE,
                rows_in     = $2,
                rows_out    = $3
          WHERE id = $1",
    )
    .bind(handle.run_id)
    .bind(rows_in)
    .bind(rows_out)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark the run failed and record the error message (truncated to 8 KiB).
/// Returning `Ok(())` here doesn't mean the *worker* succeeded — it means
/// the audit write succeeded. The HTTP response shaped from `WorkerSummary`
/// reflects the original error.
pub async fn finish_err(pool: &PgPool, handle: RunHandle, error: &str) -> sqlx::Result<()> {
    let truncated = truncate_for_audit(error);
    sqlx::query(
        "UPDATE worker_runs
            SET finished_at = NOW(),
                ok          = FALSE,
                error       = $2
          WHERE id = $1",
    )
    .bind(handle.run_id)
    .bind(truncated)
    .execute(pool)
    .await?;
    Ok(())
}

/// Truncate `s` to at most `ERROR_MAX_BYTES`, snapping to a UTF-8 char
/// boundary so we never produce invalid UTF-8 in the column.
fn truncate_for_audit(s: &str) -> String {
    if s.len() <= ERROR_MAX_BYTES {
        return s.to_owned();
    }
    // Find the largest char-boundary ≤ ERROR_MAX_BYTES - marker.len().
    const MARKER: &str = "…(truncated)";
    let budget = ERROR_MAX_BYTES.saturating_sub(MARKER.len());
    let mut cut = budget;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut out = String::with_capacity(cut + MARKER.len());
    out.push_str(&s[..cut]);
    out.push_str(MARKER);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_passes_short_strings() {
        let s = "boom";
        assert_eq!(truncate_for_audit(s), s);
    }

    #[test]
    fn truncate_caps_long_strings_with_marker() {
        let s = "x".repeat(64 * 1024);
        let out = truncate_for_audit(&s);
        assert!(out.len() <= ERROR_MAX_BYTES);
        assert!(out.ends_with("…(truncated)"));
    }

    #[test]
    fn truncate_snaps_to_char_boundary() {
        // Each '✓' is 3 bytes — choose a count that places the cut exactly
        // between bytes of a multibyte char.
        let s = "✓".repeat(ERROR_MAX_BYTES); // way over budget
        let out = truncate_for_audit(&s);
        // Must still be valid UTF-8 (String guarantees this); reach the
        // marker without panicking on a non-boundary slice above.
        assert!(out.ends_with("…(truncated)"));
        assert!(out.len() <= ERROR_MAX_BYTES);
    }
}
