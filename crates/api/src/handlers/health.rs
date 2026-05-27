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
    let total = eth_tools_db::agents::count(&state.pool, None).await?;

    let mut per_chain = Vec::with_capacity(CHAINS.len());
    for c in CHAINS {
        let n = eth_tools_db::agents::count(&state.pool, Some(c.chain_id as i64)).await?;
        per_chain.push(ChainHealth {
            chain_id: c.chain_id,
            name: c.name,
            is_testnet: c.is_testnet,
            agents_indexed: n,
        });
    }

    let h = Health {
        status: "ok",
        version: env!("CARGO_PKG_VERSION"),
        agents_indexed: total,
        chains: per_chain,
    };
    Ok(Json(serde_json::to_value(h).expect("health serializes")))
}
