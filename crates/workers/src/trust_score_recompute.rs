//! Worker W7 — hourly trust-score recomputation (plan §3 row W7).
//!
//! Joins `feedback` + `endpoint_probes` + `manifests` + `agents` into
//! `trust_scores` via a single CTE statement
//! ([`eth_tools_db::trust_scores::recompute_v1`]). Scoring formula and
//! component breakdown documented in `crates/db/src/trust_scores.rs`.
//!
//! ## Why one SQL statement and not a Rust loop?
//!
//! At steady-state we expect ≥ 10⁵ agents. Pulling all of `feedback`,
//! `endpoint_probes`, and `manifests` into Rust to compute the join in
//! application code would (a) burn the Vercel function memory budget
//! and (b) race the writers — by the time the worker writes back, the
//! source data is stale. The single CTE evaluates atomically against a
//! consistent snapshot and writes via `INSERT ... ON CONFLICT DO UPDATE`.
//!
//! ## Zero-signal agents are NOT scored
//!
//! See [`eth_tools_db::trust_scores`] module docs — the `WHERE has_signal`
//! filter excludes agents with no feedback/probes/manifests. If you want
//! "every registered agent gets a row," bump the scorer version and
//! remove the filter; existing v1 rows stay around for client migration.

use eth_tools_db::trust_scores;
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "trust_score_recompute";

/// Entrypoint called from `api/cron/trust_score_recompute.rs`.
///
/// Returns a summary where `rows_out` is the number of `trust_scores`
/// rows written (insert OR update — Postgres's `RETURNING` counts both).
/// `rows_in` is left at 0; the CTE doesn't tell us how many source rows
/// it touched and counting them would require a separate scan.
pub async fn run(ctx: &WorkerContext) -> Result<WorkerSummary, Error> {
    if ctx.dryrun {
        // Dry-run: the worker's job is to UPSERT, so there's nothing
        // meaningful to do without writing. Report ok + 0 rows.
        tracing::info!(worker = WORKER_NAME, "dryrun: skipping recompute");
        return Ok(WorkerSummary {
            worker: WORKER_NAME,
            ok: true,
            rows_in: 0,
            rows_out: 0,
            dryrun: true,
            skipped: false,
            reason: None,
        });
    }

    let rows = trust_scores::recompute_v1(&ctx.pool)
        .await
        .map_err(|e| Error::from(format!("trust_scores::recompute_v1 failed: {e}")))?;

    tracing::info!(
        worker = WORKER_NAME,
        rows_written = rows,
        "trust score recompute complete"
    );

    Ok(WorkerSummary {
        worker: WORKER_NAME,
        ok: true,
        rows_in: 0,
        rows_out: rows,
        dryrun: false,
        skipped: false,
        reason: None,
    })
}
