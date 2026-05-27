//! `health` — single-shot liveness probe. Mirrors `GET /api/v1/health` but
//! includes `staleness_ms` so an MCP client can decide whether the index is
//! fresh enough for their use case.

use super::{DataEnvelope, ToolError};
use chrono::Utc;
use eth_tools_core::CHAINS;
use eth_tools_db::Pool;
use schemars::JsonSchema;
use serde::Serialize;

#[derive(Debug, Serialize, JsonSchema)]
pub struct HealthDto {
    pub status: &'static str,
    pub version: &'static str,
    pub agents_indexed: i64,
    pub chains: Vec<ChainHealthDto>,
    /// Best-effort RPC liveness. Empty until Phase-6 wires the rotating
    /// provider stats; clients should treat absence as "no signal", not
    /// "broken".
    pub rpc: serde_json::Value,
    /// Wallclock age of the most-recently-updated row across all chains,
    /// in milliseconds. `None` if the index is empty.
    pub staleness_ms: Option<i64>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ChainHealthDto {
    pub chain_id: u64,
    pub name: &'static str,
    pub is_testnet: bool,
    pub agents_indexed: i64,
}

pub async fn health(pool: &Pool) -> Result<DataEnvelope<HealthDto>, ToolError> {
    let grouped = eth_tools_db::agents::count_by_chain(pool).await?;
    let total: i64 = grouped.iter().map(|(_, n)| *n).sum();

    let per_chain: Vec<ChainHealthDto> = CHAINS
        .iter()
        .map(|c| {
            let n = grouped
                .iter()
                .find(|(id, _)| *id == c.chain_id as i64)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            ChainHealthDto {
                chain_id: c.chain_id,
                name: c.name,
                is_testnet: c.is_testnet,
                agents_indexed: n,
            }
        })
        .collect();

    // Staleness: most-recent updated_at vs now. One COUNT query above + one
    // MAX query here = two round-trips total, which is fine for a liveness
    // endpoint (cron-scraped, not per-request hammered).
    let max_updated: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT MAX(updated_at) FROM agents")
            .fetch_one(pool)
            .await?;
    let staleness_ms = max_updated.map(|ts| (Utc::now() - ts).num_milliseconds());

    Ok(DataEnvelope::db(HealthDto {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        agents_indexed: total,
        chains: per_chain,
        // TODO(phase-6): plumb `eth_tools_rpc::provider().stats()` once
        // the RotatingProvider exposes a thread-safe snapshot.
        rpc: serde_json::json!({}),
        staleness_ms,
    }))
}
