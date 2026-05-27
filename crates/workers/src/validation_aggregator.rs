//! Worker W5 — validation aggregator (plan §3 row W5).
//!
//! Scans the ERC-8004 Validation registry events on a 5-minute cron and
//! upserts validation rows into Postgres. Two event topics per tick:
//!
//!   1. [`ValidationRequest`]  → INSERT INTO validations ON CONFLICT DO UPDATE
//!   2. [`ValidationResponse`] → INSERT…ON CONFLICT (upsert, latest-status)
//!
//! ## Latest-status-only (plan §3 W5)
//!
//! The `validations` PK is `(chain_id, request_hash)` — by definition we
//! keep one row per request, holding the most recent state. A second
//! `validationResponse` to the same request from the same validator
//! overwrites in place. History is not kept in this table; if it ever
//! becomes a requirement we'd add a `validations_history` audit table
//! mirroring `agents_history`.
//!
//! ## Spec note on `tag`
//!
//! The task description claims `ValidationRequest` carries a `tag` field.
//! The canonical contract source
//! (ValidationRegistryUpgradeable.sol, master 2026-05-15) emits `tag` on
//! the **Response** event, not the Request. We follow the contract:
//! `request_uri` is set on Request, `tag` lands on Response.
//!
//! ## Reorg safety, cursor, idempotency
//!
//! Identical pattern to W1/W4: target = head − [`REORG_SAFETY_BLOCKS`],
//! one cursor per `(chain_id, contract, topic0)`, page = 2 000 blocks,
//! per-page TX with cursor advance.

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use bigdecimal::BigDecimal;
use chrono::{DateTime, TimeZone, Utc};
use eth_tools_core::events::{
    ValidationRequest, ValidationResponse, VALIDATION_REQUEST_TOPIC, VALIDATION_RESPONSE_TOPIC,
};
use eth_tools_db::{cursors, validations};
use eth_tools_rpc::{confirmed_head, RotatingProvider, MAX_LOGS_PER_CALL, REORG_SAFETY_BLOCKS};
use sqlx::{PgPool, Postgres, Transaction};
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "validation_aggregator";

const LAG_WARN_BLOCKS: u64 = 50_000;

const SCRAPED_TOPICS: &[(&str, B256)] = &[
    ("ValidationRequest", VALIDATION_REQUEST_TOPIC),
    ("ValidationResponse", VALIDATION_RESPONSE_TOPIC),
];

/// Public entrypoint for `api/cron/validation_aggregator.rs`.
pub async fn run(ctx: &WorkerContext, chain_id: u64) -> Result<WorkerSummary, Error> {
    let chain = eth_tools_core::chains::by_id(chain_id)
        .ok_or_else(|| Error::from(format!("unknown chain_id: {chain_id}")))?;
    let validation_registry = Address::from(chain.validation_registry);

    let head = ctx
        .rpc
        .get_block_number()
        .await
        .map_err(|e| Error::from(format!("get_block_number failed: {e}")))?;
    let target = confirmed_head(head, REORG_SAFETY_BLOCKS);

    tracing::info!(
        worker = WORKER_NAME,
        chain_id,
        head,
        target,
        reorg_buffer = REORG_SAFETY_BLOCKS,
        "validation_aggregator tick start"
    );

    if target == 0 {
        return Ok(WorkerSummary::ok(WORKER_NAME));
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;

    for (event_name, topic0) in SCRAPED_TOPICS {
        let (rows_in, rows_out) = scrape_one_topic(
            ctx,
            chain.chain_id as i64,
            validation_registry,
            *topic0,
            event_name,
            target,
        )
        .await?;
        total_in = total_in.saturating_add(rows_in);
        total_out = total_out.saturating_add(rows_out);
    }

    Ok(WorkerSummary {
        worker: WORKER_NAME,
        ok: true,
        rows_in: total_in,
        rows_out: total_out,
        dryrun: ctx.dryrun,
        skipped: false,
        reason: None,
    })
}

async fn scrape_one_topic(
    ctx: &WorkerContext,
    chain_id: i64,
    contract: Address,
    topic0: B256,
    event_name: &str,
    target: u64,
) -> Result<(u64, u64), Error> {
    let contract_bytes: [u8; 20] = contract.into_array();
    let topic_bytes: [u8; 32] = topic0.into();
    let cursor_key = cursors::cursor_key(chain_id, &contract_bytes, &topic_bytes);

    let from_block = {
        let mut conn = ctx
            .pool
            .acquire()
            .await
            .map_err(|e| Error::from(format!("pool acquire failed: {e}")))?;
        let cursor = cursors::get(&mut conn, &cursor_key)
            .await
            .map_err(|e| Error::from(format!("cursors::get({cursor_key}) failed: {e}")))?;
        cursor.map(|c| (c.last_block as u64).saturating_add(1)).unwrap_or(0)
    };
    let from_block = from_block.max(chain_genesis(chain_id));

    if from_block > target {
        tracing::debug!(
            worker = WORKER_NAME,
            event = event_name,
            from_block,
            target,
            "nothing new to scrape"
        );
        return Ok((0, 0));
    }

    let lag = target.saturating_sub(from_block);
    if lag > LAG_WARN_BLOCKS {
        tracing::warn!(
            worker = WORKER_NAME,
            event = event_name,
            from_block,
            target,
            lag_blocks = lag,
            "scraper is lagging — investigate RPC quotas or cron pause"
        );
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut cursor = from_block;
    while cursor <= target {
        let page_end = cursor.saturating_add(MAX_LOGS_PER_CALL - 1).min(target);
        let logs = fetch_page(&ctx.rpc, contract, topic0, cursor, page_end).await?;
        total_in = total_in.saturating_add(logs.len() as u64);

        let written = if ctx.dryrun {
            tracing::info!(
                worker = WORKER_NAME,
                event = event_name,
                from_block = cursor,
                to_block = page_end,
                log_count = logs.len(),
                "dryrun: skipping commit"
            );
            0u64
        } else {
            commit_page(&ctx.pool, chain_id, event_name, &logs, &cursor_key, page_end).await?
        };
        total_out = total_out.saturating_add(written);

        cursor = match page_end.checked_add(1) {
            Some(c) => c,
            None => break,
        };
    }
    Ok((total_in, total_out))
}

async fn fetch_page(
    rpc: &Arc<RotatingProvider>,
    contract: Address,
    topic0: B256,
    from_block: u64,
    to_block: u64,
) -> Result<Vec<Log>, Error> {
    let filter = Filter::new()
        .address(contract)
        .event_signature(topic0)
        .from_block(from_block)
        .to_block(to_block);

    rpc.get_logs_paginated(filter, from_block, to_block, MAX_LOGS_PER_CALL)
        .await
        .map_err(|e| Error::from(format!("get_logs_paginated failed: {e}")))
}

async fn commit_page(
    pool: &PgPool,
    chain_id: i64,
    event_name: &str,
    logs: &[Log],
    cursor_key: &str,
    page_end: u64,
) -> Result<u64, Error> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| Error::from(format!("tx begin failed: {e}")))?;

    let mut last_log_index_i32: i32 = 0;
    let mut written: u64 = 0;

    for log in logs {
        let log_index = log.log_index.unwrap_or(0);
        last_log_index_i32 = log_index.try_into().unwrap_or(i32::MAX);

        match apply_log(&mut tx, chain_id, event_name, log).await {
            Ok(true) => written = written.saturating_add(1),
            Ok(false) => {}
            Err(e) => {
                tracing::error!(
                    worker = WORKER_NAME,
                    event = event_name,
                    chain_id,
                    block_number = log.block_number.unwrap_or(0),
                    log_index,
                    error = %e,
                    "apply_log failed; rolling back page"
                );
                let _ = tx.rollback().await;
                return Err(e);
            }
        }
    }

    cursors::advance(&mut tx, cursor_key, page_end as i64, last_log_index_i32)
        .await
        .map_err(|e| Error::from(format!("cursors::advance failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| Error::from(format!("tx commit failed: {e}")))?;

    Ok(written)
}

async fn apply_log(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    event_name: &str,
    log: &Log,
) -> Result<bool, Error> {
    let block_number = log
        .block_number
        .ok_or_else(|| Error::from("log missing block_number"))?;
    let _ = block_number;
    let block_ts = block_timestamp_or_now(log);

    match event_name {
        "ValidationRequest" => {
            let decoded = ValidationRequest::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode ValidationRequest failed: {e}")))?;
            let validator_bytes = decoded.validatorAddress.0.to_vec();
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let request_uri = decoded.requestURI.clone();
            let request_hash: Vec<u8> = decoded.requestHash.0.to_vec();

            validations::upsert_request(
                tx,
                chain_id,
                &request_hash,
                &validator_bytes,
                &agent_id,
                &request_uri,
                block_ts,
            )
            .await
            .map_err(|e| Error::from(format!("validations::upsert_request failed: {e}")))?;
            Ok(true)
        }
        "ValidationResponse" => {
            let decoded = ValidationResponse::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode ValidationResponse failed: {e}")))?;
            let validator_bytes = decoded.validatorAddress.0.to_vec();
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let request_hash: Vec<u8> = decoded.requestHash.0.to_vec();
            // uint8, 0..=100 enforced on-chain.
            let response_i16 = decoded.response as i16;
            let response_uri = nonempty(decoded.responseURI.clone());
            let response_hash: Vec<u8> = decoded.responseHash.0.to_vec();
            let tag = nonempty(decoded.tag.clone());

            validations::upsert_response(
                tx,
                chain_id,
                &request_hash,
                &validator_bytes,
                &agent_id,
                response_i16,
                response_uri.as_deref(),
                Some(&response_hash),
                tag.as_deref(),
                block_ts,
            )
            .await
            .map_err(|e| Error::from(format!("validations::upsert_response failed: {e}")))?;
            Ok(true)
        }
        _ => {
            tracing::error!(worker = WORKER_NAME, event = event_name, "unknown event");
            Ok(false)
        }
    }
}

fn nonempty(s: String) -> Option<String> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn block_timestamp_or_now(log: &Log) -> DateTime<Utc> {
    log.block_timestamp
        .and_then(|ts| Utc.timestamp_opt(ts as i64, 0).single())
        .unwrap_or_else(Utc::now)
}

fn u256_to_bigdecimal(value: U256) -> BigDecimal {
    value
        .to_string()
        .parse::<BigDecimal>()
        .expect("U256 decimal string always parses as BigDecimal")
}

fn chain_genesis(chain_id: i64) -> u64 {
    eth_tools_core::chains::by_id(chain_id as u64)
        .map(|c| c.genesis_block)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scraped_topics_are_distinct_and_nonzero() {
        for (i, (_, a)) in SCRAPED_TOPICS.iter().enumerate() {
            assert_ne!(*a, B256::ZERO, "topic {i} is zero");
            for (j, (_, b)) in SCRAPED_TOPICS.iter().enumerate() {
                if i != j {
                    assert_ne!(*a, *b, "topic {i} == topic {j}");
                }
            }
        }
    }

    #[test]
    fn nonempty_drops_empty_strings() {
        assert_eq!(nonempty(String::new()), None);
        assert_eq!(nonempty("x".into()), Some("x".to_string()));
    }

    #[test]
    fn chain_genesis_uses_core_constant() {
        assert_eq!(chain_genesis(8453), 41_663_783);
        assert_eq!(chain_genesis(99_999), 0);
    }
}
