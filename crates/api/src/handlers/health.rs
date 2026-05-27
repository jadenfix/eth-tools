//! `GET /api/v1/health` — minimal liveness so the dashboard / status page
//! and external uptime monitors have a stable contract.
//!
//! Phase-1 fields: chain registry rows, count of indexed agents per chain.
//! Phase-4 will add: per-chain `cursor_lag_blocks`, RPC rolling success rate.

use axum::extract::State;
use axum::Json;
use eth_tools_core::CHAINS;
use serde::Serialize;
use serde_json::Value;

use crate::error::ApiError;
use crate::AppState;

#[derive(Serialize)]
pub struct ChainHealth {
    pub chain_id: u64,
    pub name: &'static str,
    pub is_testnet: bool,
    pub agents_indexed: i64,
}

#[derive(Serialize)]
pub struct Health {
    pub status: &'static str,
    pub version: &'static str,
    pub agents_indexed: i64,
    pub chains: Vec<ChainHealth>,
}

pub async fn get(State(state): State<AppState>) -> Result<Json<Value>, ApiError> {
    // Single grouped query instead of 1 + N per-chain COUNT(*). `/health` is
    // the canonical liveness probe — uptime monitors hit it often, so an N+1
    // here turns into a self-inflicted small DoS on the pooler.
    let grouped = eth_tools_db::agents::count_by_chain(&state.pool).await?;
    let total: i64 = grouped.iter().map(|(_, n)| *n).sum();

    let per_chain: Vec<ChainHealth> = CHAINS
        .iter()
        .map(|c| {
            let n = grouped
                .iter()
                .find(|(id, _)| *id == c.chain_id as i64)
                .map(|(_, n)| *n)
                .unwrap_or(0);
            ChainHealth {
                chain_id: c.chain_id,
                name: c.name,
                is_testnet: c.is_testnet,
                agents_indexed: n,
            }
        })
        .collect();

    let h = Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        agents_indexed: total,
        chains: per_chain,
    };
    Ok(Json(serde_json::to_value(h).expect("health serializes")))
}
