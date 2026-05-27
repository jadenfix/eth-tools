//! The 8 autonomous worker agents (plan §3). Each cron function under
//! `api/cron/*.rs` is a thin wrapper that calls one of:
//!   - `cron::serve(name, req, |dryrun| …)` — legacy PR1 entrypoint, used
//!     by the 8 stubs today.
//!   - `cron::serve_with_context(name, req, |ctx| …)` — **M1 refactor
//!     from PR2.** Closure receives a [`WorkerContext`] (pool, rpc,
//!     vercel_env, dryrun, force). PR3 migrates the 8 stubs to this
//!     entrypoint and deletes the legacy adapter.
//!
//! Both enforce the per-worker invariants from plan §3:
//!  - **Cron secret check** — fail-closed in production if `CRON_SECRET` is
//!    unset.
//!  - **Env-aware skip** — non-production environments no-op (plan §11.5),
//!    unless `?force=1` (plan §3.2: backfill/replay opt-in).
//!  - **Dry-run mode** via `?dryrun=1`.
//!  - **Telemetry** — `serve_with_context` writes a `worker_runs` audit
//!    row per non-dryrun invocation via `eth_tools_db::worker_runs`.
//!
//! Real worker bodies (scraping, fetching, probing) land in PR3+.

use serde::Serialize;

pub mod context;
pub mod cron;
pub mod manifest_fetcher;
mod rpc_http;

pub use context::{deps, WorkerContext, WorkerDeps};

#[derive(Debug, Serialize)]
pub struct WorkerSummary {
    pub worker: &'static str,
    pub ok: bool,
    pub rows_in: u64,
    pub rows_out: u64,
    pub dryrun: bool,
    pub skipped: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
}

impl WorkerSummary {
    pub fn ok(name: &'static str) -> Self {
        Self {
            worker: name,
            ok: true,
            rows_in: 0,
            rows_out: 0,
            dryrun: false,
            skipped: false,
            reason: None,
        }
    }
    pub fn skipped(name: &'static str, reason: &'static str) -> Self {
        Self {
            worker: name,
            ok: true,
            rows_in: 0,
            rows_out: 0,
            dryrun: false,
            skipped: true,
            reason: Some(reason),
        }
    }
    pub fn dryrun(mut self) -> Self {
        self.dryrun = true;
        self
    }
}

pub const ALL_WORKERS: &[&str] = &[
    "registry_scraper",
    "manifest_fetcher",
    "endpoint_prober",
    "reputation_aggregator",
    "validation_aggregator",
    "wallet_rotation_watcher",
    "trust_score_recompute",
    "wallet_balance_keeper",
];

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn worker_count_matches_plan() {
        assert_eq!(ALL_WORKERS.len(), 8);
    }
}
