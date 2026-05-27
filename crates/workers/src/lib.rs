//! The 8 autonomous worker agents (plan §3). Each cron function under
//! `api/cron/*.rs` is a thin wrapper that calls `cron::serve(name, req, body)`.
//!
//! `cron::serve` enforces the per-worker invariants from plan §3:
//!  - **Cron secret check** — fail-closed in production if `CRON_SECRET` is unset
//!    (this is the security hole the deep review caught).
//!  - **Env-aware skip** — non-production environments no-op (plan §11.5).
//!  - **Dry-run mode** via `?dryrun=1`.
//!  - **Telemetry** — `WorkerSummary` is the wire shape for `worker_runs`.
//!
//! Real worker bodies (scraping, fetching, probing) land in Phase 4.

use serde::Serialize;

pub mod cron;

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
