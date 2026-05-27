//! `/api/v1/reputation/*`.
//!
//! - `GET /api/v1/reputation/:chain/:id` — list every `feedback` row indexed
//!   for an agent (read-side, anonymous + rate-limited).
//! - `POST /api/v1/reputation/give` — submit a new feedback. STUBBED: the
//!   write path goes through the wallet rails (`feat/phase-6-wallet-rails`).
//!   Returns 503 until that branch lands, but the OpenAPI shape is committed
//!   now so clients can integrate against the spec.

use axum::extract::{Path, State};
use axum::Json;
use bigdecimal::BigDecimal;
use eth_tools_db::feedback;
use std::str::FromStr;

use crate::dto::{FeedbackDto, ReputationGiveRequest, ReputationList};
use crate::error::ApiError;
use crate::handlers::agents::resolve_chain;
use crate::AppState;

#[utoipa::path(
    get,
    path = "/api/v1/reputation/{chain}/{agent_id}",
    tag = "reputation",
    operation_id = "reputation_read",
    params(
        ("chain" = String, Path, description = "Chain name or chain_id"),
        ("agent_id" = String, Path, description = "uint256 as decimal string"),
    ),
    responses(
        (status = 200, description = "List feedback rows for an agent, newest first", body = crate::dto::ReputationList),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 404, description = "chain not found", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 500, description = "internal error", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn read(
    State(state): State<AppState>,
    Path((chain, agent_id)): Path<(String, String)>,
) -> Result<Json<ReputationList>, ApiError> {
    let chain = resolve_chain(&chain)?;
    let id = BigDecimal::from_str(&agent_id).map_err(|_| ApiError::InvalidAgentId)?;
    let rows = feedback::list_for_agent(&state.pool, chain.chain_id as i64, &id, 200).await?;
    let data: Vec<FeedbackDto> = rows.into_iter().map(FeedbackDto::from_row).collect();
    Ok(Json(ReputationList {
        data,
        staleness_ms: 0,
        source: "db".into(),
    }))
}

#[utoipa::path(
    post,
    path = "/api/v1/reputation/give",
    tag = "reputation",
    operation_id = "reputation_give",
    request_body = crate::dto::ReputationGiveRequest,
    responses(
        (status = 200, description = "Submitted reputation feedback", body = crate::dto::ApiErrorBody),
        (status = 400, description = "bad input", body = crate::dto::ApiErrorBody),
        (status = 401, description = "missing bearer token", body = crate::dto::ApiErrorBody),
        (status = 402, description = "x402 payment required", body = crate::dto::ApiErrorBody),
        (status = 429, description = "rate limited", body = crate::dto::ApiErrorBody),
        (status = 503, description = "wallet rails not yet wired (feat/phase-6-wallet-rails)", body = crate::dto::ApiErrorBody),
    )
)]
pub async fn give(
    State(_state): State<AppState>,
    Json(_req): Json<ReputationGiveRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    // TODO(feat/phase-6-wallet-rails): build calldata for
    // ReputationRegistry.giveFeedback(...) and hand to the signing rails. The
    // contract here is "execute or queue, return tx hash + DeniedReason on
    // bounce".
    Err(ApiError::NotImplemented("feat/phase-6-wallet-rails"))
}
