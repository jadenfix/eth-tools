//! `GET /api/v1/agents` (list, keyset-paginated) and
//! `GET /api/v1/agents/:chain/:agent_id` (single).
//!
//! `chain` accepts both name (`base`) and chain_id (`8453`). `agent_id` is a
//! decimal uint256 string. Cursor is opaque base64url(json).

use axum::extract::{Path, Query, State};
use axum::Json;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_core::chains;
use eth_tools_db::agents::{self, KeysetCursor, ListParams};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

use crate::dto::{AgentDto, ListEnvelope, OneEnvelope};
use crate::error::ApiError;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    #[serde(default = "default_limit")]
    pub limit: i64,
    pub chain: Option<String>,
    pub cursor: Option<String>,
}

fn default_limit() -> i64 {
    50
}

#[derive(Debug, Serialize, Deserialize)]
struct CursorPayload {
    updated_at: DateTime<Utc>,
    chain_id: i64,
    agent_id: String,
}

fn encode_cursor(row: &eth_tools_db::agents::AgentRow) -> String {
    let p = CursorPayload {
        updated_at: row.updated_at,
        chain_id: row.chain_id,
        agent_id: row
            .agent_id
            .to_string()
            .split('.')
            .next()
            .unwrap_or("")
            .to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&p).expect("cursor serializes"))
}

fn decode_cursor(s: &str) -> Result<KeysetCursor, ApiError> {
    let bytes = URL_SAFE_NO_PAD.decode(s).map_err(|_| ApiError::InvalidCursor)?;
    let p: CursorPayload = serde_json::from_slice(&bytes).map_err(|_| ApiError::InvalidCursor)?;
    let agent_id = BigDecimal::from_str(&p.agent_id).map_err(|_| ApiError::InvalidCursor)?;
    Ok(KeysetCursor {
        updated_at: p.updated_at,
        chain_id: p.chain_id,
        agent_id,
    })
}

pub async fn list(
    State(state): State<AppState>,
    Query(q): Query<ListQuery>,
) -> Result<Json<ListEnvelope<AgentDto>>, ApiError> {
    let chain_id = match q.chain.as_deref() {
        None => None,
        Some(s) => Some(resolve_chain(s)?.chain_id as i64),
    };
    let after = match q.cursor {
        None => None,
        Some(c) => Some(decode_cursor(&c)?),
    };
    // Confused-deputy guard: if the caller pins a chain AND replays a cursor,
    // the cursor's chain MUST match the resolved chain. Otherwise a client can
    // page `?chain=base` to get a cursor, then resubmit it with
    // `?chain=base-sepolia&cursor=…` and the WHERE clause's `chain_id = $1`
    // filter would interact with the keyset predicate in confusing ways. Today
    // no per-chain ACL exists; rejecting the mismatch keeps the API honest
    // before ACLs land.
    if let (Some(want), Some(cur)) = (chain_id, after.as_ref()) {
        if cur.chain_id != want {
            return Err(ApiError::InvalidCursor);
        }
    }
    let limit = q.limit.clamp(1, 200);

    // Request limit+1 so we know whether there's a next page without a
    // second query.
    let rows = agents::list(
        &state.pool,
        ListParams {
            chain_id,
            limit: limit + 1,
            after,
        },
    )
    .await?;

    let data: Vec<AgentDto> = rows
        .iter()
        .take(limit as usize)
        .map(|r| {
            let chain_name = chains::by_id(r.chain_id as u64)
                .map(|c| c.name)
                .unwrap_or("unknown");
            AgentDto::from_row(r.clone(), chain_name)
        })
        .collect();

    // We over-fetched by 1 to detect "more pages". The cursor is computed
    // from the last row WE RETURN (not the over-fetched probe row).
    let next_cursor = if rows.len() > limit as usize {
        rows.get((limit as usize) - 1).map(encode_cursor)
    } else {
        None
    };

    Ok(Json(ListEnvelope {
        data,
        next_cursor,
        staleness_ms: 0, // Phase-4: real `now - cursors.updated_at`.
        source: "db",
    }))
}

pub async fn get_one(
    State(state): State<AppState>,
    Path((chain, agent_id)): Path<(String, String)>,
) -> Result<Json<OneEnvelope<AgentDto>>, ApiError> {
    let chain = resolve_chain(&chain)?;
    let id = BigDecimal::from_str(&agent_id).map_err(|_| ApiError::InvalidAgentId)?;

    let row = agents::get_one(&state.pool, chain.chain_id as i64, &id)
        .await?
        .ok_or(ApiError::AgentNotFound)?;
    Ok(Json(OneEnvelope {
        data: AgentDto::from_row(row, chain.name),
        staleness_ms: 0,
        source: "db",
    }))
}

/// Accept chain by name (`base`) or id (`8453`). Returns 404 envelope if
/// neither matches.
fn resolve_chain(s: &str) -> Result<&'static eth_tools_core::Chain, ApiError> {
    if let Ok(id) = s.parse::<u64>() {
        if let Some(c) = chains::by_id(id) {
            return Ok(c);
        }
    }
    chains::by_name(s).ok_or(ApiError::ChainNotFound)
}
