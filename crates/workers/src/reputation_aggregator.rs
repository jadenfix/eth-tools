//! Worker W4 — reputation aggregator (plan §3 row W4).
//!
//! Scans the ERC-8004 Reputation registry events on a 5-minute cron and
//! upserts feedback / response rows into Postgres. Three event topics per
//! tick:
//!
//!   1. [`NewFeedback`]      → INSERT INTO feedback ON CONFLICT DO UPDATE
//!   2. [`FeedbackRevoked`]  → UPDATE feedback SET is_revoked = TRUE
//!   3. [`ResponseAppended`] → INSERT INTO feedback_responses
//!
//! ## Rolling summary cache (plan §3 W4)
//!
//! After committing each page we compute a per-agent summary
//! `(count, mode_decimals, mean_value)` via [`feedback::compute_summary`]
//! and best-effort push it to Upstash KV with a 5-minute TTL. Cache
//! failures NEVER fail the worker — the canonical store is Postgres.
//!
//! ### "Mode of decimals" interpretation
//!
//! On-chain feedback carries an explicit `valueDecimals` per row (0..18).
//! Two clients can report `(value=50, decimals=2)` (=> 0.50) and
//! `(value=5000, decimals=4)` (=> 0.50) — the magnitudes mean different
//! things across scales. The W4 spec asks for "mode-of-decimals: if some
//! feedback has decimals=2 and some =4, the mode is the majority; values
//! normalized accordingly". We resolve this by:
//!   1. Computing the mode of `value_decimals` across all non-revoked
//!      feedback for the agent (tie-break: larger decimals wins,
//!      preserving precision when bimodal).
//!   2. Computing `mean_value` only over rows whose `value_decimals`
//!      equals that mode — i.e. we **filter** the minority scales out
//!      rather than rescaling them, because rescaling 50 with decimals=2
//!      to decimals=4 (=> 5000) and then averaging is mathematically the
//!      same as treating them as the same scale, but rescaling losses
//!      reappear when decimals diverge wildly (a decimals=18 value with
//!      value=1 represents 1e-18; rescaling to decimals=2 would lose all
//!      precision). Filtering is simpler, defensible, and preserves the
//!      raw scale of the majority of clients.
//!
//! ## Reorg safety, cursor, idempotency
//!
//! Identical to W1: target = head − [`REORG_SAFETY_BLOCKS`], one cursor
//! per `(chain_id, contract, topic0)`, page = 2 000 blocks, per-page TX
//! with cursor advance. See [`crate::registry_scraper`] for the pattern
//! prose. The substrate-level `worker_runs` audit row is written by
//! `cron::serve_with_context` around our body — we do NOT touch
//! `worker_runs` here.

use std::collections::BTreeSet;
use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use bigdecimal::BigDecimal;
use chrono::{DateTime, TimeZone, Utc};
use eth_tools_core::events::{
    FeedbackRevoked, NewFeedback, ResponseAppended, FEEDBACK_REVOKED_TOPIC, NEW_FEEDBACK_TOPIC,
    RESPONSE_APPENDED_TOPIC,
};
use eth_tools_db::{cursors, feedback};
use eth_tools_rpc::{confirmed_head, RotatingProvider, MAX_LOGS_PER_CALL, REORG_SAFETY_BLOCKS};
use sqlx::{PgPool, Postgres, Transaction};
use vercel_runtime::Error;

use crate::reputation_summary;
use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "reputation_aggregator";

/// Spec-stipulated max gap between cursor and target before we log a
/// warning. 50 000 blocks ≈ 28 hours on Base.
const LAG_WARN_BLOCKS: u64 = 50_000;

const SCRAPED_TOPICS: &[(&str, B256)] = &[
    ("NewFeedback", NEW_FEEDBACK_TOPIC),
    ("FeedbackRevoked", FEEDBACK_REVOKED_TOPIC),
    ("ResponseAppended", RESPONSE_APPENDED_TOPIC),
];

/// Public entrypoint for `api/cron/reputation_aggregator.rs`.
pub async fn run(ctx: &WorkerContext, chain_id: u64) -> Result<WorkerSummary, Error> {
    let chain = eth_tools_core::chains::by_id(chain_id)
        .ok_or_else(|| Error::from(format!("unknown chain_id: {chain_id}")))?;
    let reputation_registry = Address::from(chain.reputation_registry);

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
        "reputation_aggregator tick start"
    );

    if target == 0 {
        return Ok(WorkerSummary::ok(WORKER_NAME));
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    // Accumulator of touched agents across all topics; we recompute their
    // summaries once at the end so a single agent that received both a
    // NewFeedback and a Revoke in this tick is only summarized once.
    let mut touched_agents: BTreeSet<BigDecimal> = BTreeSet::new();

    for (event_name, topic0) in SCRAPED_TOPICS {
        let (rows_in, rows_out, agents) = scrape_one_topic(
            ctx,
            chain.chain_id as i64,
            reputation_registry,
            *topic0,
            event_name,
            target,
        )
        .await?;
        total_in = total_in.saturating_add(rows_in);
        total_out = total_out.saturating_add(rows_out);
        touched_agents.extend(agents);
    }

    // Best-effort rolling summary refresh + Upstash cache. Errors are
    // swallowed — Postgres is the source of truth.
    if !ctx.dryrun {
        for agent_id in &touched_agents {
            refresh_summary(ctx, chain.chain_id as i64, agent_id).await;
        }
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

async fn refresh_summary(ctx: &WorkerContext, chain_id: i64, agent_id: &BigDecimal) {
    match feedback::compute_summary(&ctx.pool, chain_id, agent_id).await {
        Ok(Some(summary)) => {
            if let Err(e) = reputation_summary::cache_summary(&summary).await {
                tracing::debug!(
                    worker = WORKER_NAME,
                    agent_id = %agent_id,
                    error = %e,
                    "summary cache serialization failed; ignoring"
                );
            }
        }
        Ok(None) => {
            // Agent has zero non-revoked feedback; nothing to cache.
        }
        Err(e) => {
            tracing::warn!(
                worker = WORKER_NAME,
                chain_id,
                agent_id = %agent_id,
                error = %e,
                "compute_summary failed; skipping cache refresh"
            );
        }
    }
}

async fn scrape_one_topic(
    ctx: &WorkerContext,
    chain_id: i64,
    contract: Address,
    topic0: B256,
    event_name: &str,
    target: u64,
) -> Result<(u64, u64, BTreeSet<BigDecimal>), Error> {
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
        return Ok((0, 0, BTreeSet::new()));
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
    let mut touched: BTreeSet<BigDecimal> = BTreeSet::new();
    let mut cursor = from_block;
    while cursor <= target {
        let page_end = cursor.saturating_add(MAX_LOGS_PER_CALL - 1).min(target);
        let logs = fetch_page(&ctx.rpc, contract, topic0, cursor, page_end).await?;
        total_in = total_in.saturating_add(logs.len() as u64);

        let (written, agents) = if ctx.dryrun {
            tracing::info!(
                worker = WORKER_NAME,
                event = event_name,
                from_block = cursor,
                to_block = page_end,
                log_count = logs.len(),
                "dryrun: skipping commit"
            );
            (0u64, BTreeSet::new())
        } else {
            commit_page(&ctx.pool, chain_id, event_name, &logs, &cursor_key, page_end).await?
        };
        total_out = total_out.saturating_add(written);
        touched.extend(agents);

        cursor = match page_end.checked_add(1) {
            Some(c) => c,
            None => break,
        };
    }
    Ok((total_in, total_out, touched))
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
) -> Result<(u64, BTreeSet<BigDecimal>), Error> {
    let mut tx = pool
        .begin()
        .await
        .map_err(|e| Error::from(format!("tx begin failed: {e}")))?;

    let mut last_log_index_i32: i32 = 0;
    let mut written: u64 = 0;
    let mut touched: BTreeSet<BigDecimal> = BTreeSet::new();

    for log in logs {
        let log_index = log.log_index.unwrap_or(0);
        last_log_index_i32 = log_index.try_into().unwrap_or(i32::MAX);

        match apply_log(&mut tx, chain_id, event_name, log).await {
            Ok(Some(agent_id)) => {
                written = written.saturating_add(1);
                touched.insert(agent_id);
            }
            Ok(None) => {}
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

    Ok((written, touched))
}

/// Dispatch one log. Returns `Ok(Some(agent_id))` for a row that touched
/// the canonical state (so the caller can refresh that agent's summary);
/// `Ok(None)` for no-op (out-of-order revoke / unknown event name).
async fn apply_log(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    event_name: &str,
    log: &Log,
) -> Result<Option<BigDecimal>, Error> {
    let block_number = log
        .block_number
        .ok_or_else(|| Error::from("log missing block_number"))?;
    let log_index = log.log_index.unwrap_or(0) as i32;
    let tx_hash_bytes = log
        .transaction_hash
        .map(|h| h.0.to_vec())
        .unwrap_or_else(|| vec![0u8; 32]);
    let block_ts = block_timestamp_or_now(log);

    match event_name {
        "NewFeedback" => {
            let decoded = NewFeedback::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode NewFeedback failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let client_bytes = decoded.clientAddress.0.to_vec();
            // value is int128 — preserve sign through BigDecimal.
            let value_bd: BigDecimal = decoded.value.to_string().parse().map_err(|e| {
                Error::from(format!("BigDecimal parse(int128 value) failed: {e}"))
            })?;
            let value_decimals = decoded.valueDecimals as i16;
            let feedback_index = decoded.feedbackIndex as i64;
            // tag fields come through as String (non-indexed copies).
            let tag1 = nonempty(decoded.tag1.clone());
            let tag2 = nonempty(decoded.tag2.clone());
            let endpoint = nonempty(decoded.endpoint.clone());
            let feedback_uri = nonempty(decoded.feedbackURI.clone());
            let feedback_hash: Vec<u8> = decoded.feedbackHash.0.to_vec();

            feedback::upsert_new_feedback(
                tx,
                chain_id,
                &agent_id,
                &client_bytes,
                feedback_index,
                &value_bd,
                value_decimals,
                tag1.as_deref(),
                tag2.as_deref(),
                endpoint.as_deref(),
                feedback_uri.as_deref(),
                Some(&feedback_hash),
                &tx_hash_bytes,
                block_number as i64,
            )
            .await
            .map_err(|e| Error::from(format!("feedback::upsert_new_feedback failed: {e}")))?;
            // block_ts surfaced for log correlation; the feedback table
            // doesn't carry a created_at column.
            let _ = block_ts;
            let _ = log_index;
            Ok(Some(agent_id))
        }
        "FeedbackRevoked" => {
            let decoded = FeedbackRevoked::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode FeedbackRevoked failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let client_bytes = decoded.clientAddress.0.to_vec();
            let feedback_index = decoded.feedbackIndex as i64;

            let updated = feedback::mark_revoked(
                tx,
                chain_id,
                &agent_id,
                &client_bytes,
                feedback_index,
            )
            .await
            .map_err(|e| Error::from(format!("feedback::mark_revoked failed: {e}")))?;
            if !updated {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    client = ?client_bytes,
                    feedback_index,
                    "FeedbackRevoked for unknown row — NewFeedback lost? skipping"
                );
                return Ok(None);
            }
            Ok(Some(agent_id))
        }
        "ResponseAppended" => {
            let decoded = ResponseAppended::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode ResponseAppended failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let client_bytes = decoded.clientAddress.0.to_vec();
            let responder_bytes = decoded.responder.0.to_vec();
            let feedback_index = decoded.feedbackIndex as i64;
            let response_uri = nonempty(decoded.responseURI.clone());
            let response_hash: Vec<u8> = decoded.responseHash.0.to_vec();

            feedback::upsert_response_appended(
                tx,
                chain_id,
                &agent_id,
                &client_bytes,
                feedback_index,
                &responder_bytes,
                block_number as i64,
                log_index,
                response_uri.as_deref(),
                Some(&response_hash),
                &tx_hash_bytes,
                block_ts,
            )
            .await
            .map_err(|e| {
                Error::from(format!("feedback::upsert_response_appended failed: {e}"))
            })?;
            Ok(Some(agent_id))
        }
        _ => {
            tracing::error!(worker = WORKER_NAME, event = event_name, "unknown event");
            Ok(None)
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
