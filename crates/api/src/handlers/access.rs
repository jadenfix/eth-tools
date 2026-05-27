//! `/api/v1/access/{check,explain}`.
//!
//! The real policy engine lives in `crates/payments::policy` (Phase-6, gating
//! `feat/phase-6-x402` + wallet rails). For now these handlers implement the
//! deterministic v0 policy described in plan §9.1:
//!
//!   - look up the agent
//!   - allow if the requester is the agent's `owner`
//!   - allow if the agent has no `agent_wallet` set (open access pre-listing)
//!   - else deny
//!
//! That's enough for the dashboard and CLI to render a UI before the full
//! engine lands. When `feat/phase-6-x402` merges, swap the body of
//! `evaluate_v0` for the real engine without changing the wire contract.

use axum::extract::State;
use axum::Json;
use bigdecimal::BigDecimal;
use eth_tools_db::agents;
use std::str::FromStr;

use crate::dto::{AccessCheckResponse, AccessExplainResponse, AccessRequest, AccessStep};
use crate::error::ApiError;
use crate::handlers::agents::{parse_address, resolve_chain};
use crate::AppState;

struct Decision {
    allowed: bool,
    reason: String,
    steps: Vec<AccessStep>,
}

async fn evaluate_v0(
    state: &AppState,
    req: &AccessRequest,
) -> Result<Decision, ApiError> {
    let mut steps = Vec::with_capacity(3);
    // Step 1 — input parsing.
    let chain = resolve_chain(&req.chain)?;
    steps.push(AccessStep {
        check: "chain_resolved".into(),
        ok: true,
        detail: format!("{} ({})", chain.name, chain.chain_id),
    });
    let agent_id = BigDecimal::from_str(&req.agent_id).map_err(|_| ApiError::InvalidAgentId)?;
    let requester = parse_address(&req.requester_address)?;
    steps.push(AccessStep {
        check: "input_parsed".into(),
        ok: true,
        detail: format!("requester={}", req.requester_address),
    });

    // Step 2 — agent lookup.
    let row = agents::get_one(&state.pool, chain.chain_id as i64, &agent_id).await?;
    let agent = match row {
        Some(r) => r,
        None => {
            steps.push(AccessStep {
                check: "agent_lookup".into(),
                ok: false,
                detail: "agent not indexed on this chain".into(),
            });
            return Ok(Decision {
                allowed: false,
                reason: "AGENT_NOT_INDEXED".into(),
                steps,
            });
        }
    };
    steps.push(AccessStep {
        check: "agent_lookup".into(),
        ok: true,
        detail: format!("owner={}", hex(&agent.owner)),
    });

    // Step 3 — policy.
    if agent.owner == requester {
        steps.push(AccessStep {
            check: "owner_match".into(),
            ok: true,
            detail: format!("requester is owner of agent {}", req.agent_id),
        });
        return Ok(Decision {
            allowed: true,
            reason: "OWNER".into(),
            steps,
        });
    }
    if agent.agent_wallet.is_none() {
        steps.push(AccessStep {
            check: "open_access".into(),
            ok: true,
            detail: "agent_wallet is null (pre-listing); v0 policy allows".into(),
        });
        return Ok(Decision {
            allowed: true,
            reason: "OPEN_ACCESS".into(),
            steps,
        });
    }
    steps.push(AccessStep {
        check: "policy_default_deny".into(),
        ok: false,
        detail: format!(
            "v0 policy: action `{}` requires owner; real engine lands with feat/phase-6-x402",
            req.action
        ),
    });
    Ok(Decision {
        allowed: false,
        reason: "NOT_OWNER".into(),
        steps,
    })
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(2 + 2 * bytes.len());
    s.push_str("0x");
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[utoipa::path(
    post,
    path = "/api/v1/access/check",
    tag = "access",
    operation_id = "access_check",
    request_body = crate::dto::AccessRequest,
    responses(
        (status = 200, description = "Boolean allow/deny + one-line reason", body = crate::dto::AccessCheckResponse),
        (status = 400, description = "bad input (address / agent_id / chain)", body = crate::dto::ApiErrorBody),
        (status = 404, description = "chain not indexed", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn check(
    State(state): State<AppState>,
    Json(req): Json<AccessRequest>,
) -> Result<Json<AccessCheckResponse>, ApiError> {
    let d = evaluate_v0(&state, &req).await?;
    Ok(Json(AccessCheckResponse {
        allowed: d.allowed,
        reason: d.reason,
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/access/explain",
    tag = "access",
    operation_id = "access_explain",
    request_body = crate::dto::AccessRequest,
    responses(
        (status = 200, description = "Verbose policy breakdown", body = crate::dto::AccessExplainResponse),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 404, description = "chain not indexed", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn explain(
    State(state): State<AppState>,
    Json(req): Json<AccessRequest>,
) -> Result<Json<AccessExplainResponse>, ApiError> {
    let d = evaluate_v0(&state, &req).await?;
    Ok(Json(AccessExplainResponse {
        allowed: d.allowed,
        reason: d.reason,
        steps: d.steps,
    }))
}
