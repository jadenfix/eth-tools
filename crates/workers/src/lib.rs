//! The 8 autonomous worker agents (plan §3). Each cron function under
//! `api/cron/*.rs` is a thin wrapper over `run(<worker>)` defined here.
//!
//! Bootstrap version: every worker returns an "ok, no-op" summary.
//! Real scraping/probing/aggregating lands in subsequent PRs.

use serde::Serialize;

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
