//! Worker W6 — wallet-rotation watcher (plan §3 row W6 — spec gotcha #2).
//!
//! Runs every minute. Watches two event streams on the Identity registry
//! and invalidates the downstream `agent_wallet:{chain}:{agent_id}` cache
//! so a freshly-rotated wallet doesn't get a stale read for up to a full
//! W1 tick (2 minutes).
//!
//!   1. ERC-721 `Transfer(from, to, tokenId)` — clears `agent_wallet`
//!      and rotates `owner`. The spec REQUIRES `agentWallet` be cleared
//!      on transfer (a poisoned cache would let the previous owner keep
//!      receiving payments — see plan §3 W6).
//!
//!   2. `MetadataSet(agentId, key, value)` — when `key == "agentWallet"`,
//!      sets `agent_wallet` to the new value.
//!
//! ## Overlap with W1 (registry_scraper)
//!
//! W1 also scrapes these events on its 2-minute cadence and is the
//! source of truth for the `agents` row mutation. W6's job is the
//! **cache invalidation** that W1 doesn't do — it runs on a faster
//! cadence so cache-poisoning windows close inside one minute.
//!
//! ## Cursor namespace
//!
//! W6 maintains its own cursor (`cursors.cursor_key + ":w6"`) so its
//! scan doesn't rewind W1's progress. The same chain can have two
//! workers reading the same topic — they each remember where they got
//! to independently.
//!
//! ## What W6 writes
//!
//!   - UPDATE agents SET (owner, agent_wallet, …) — same UPDATE W1
//!     would have done; we run it eagerly so the cache invalidation
//!     and the row update happen inside the same minute window.
//!   - INSERT into agents_history.
//!   - kv::del("agent_wallet:{chain}:{agent_id}") — best-effort.
//!
//! Cache invalidation failures do NOT fail the worker — the chain-level
//! invariant (DB row updated) is already satisfied; the cache is just a
//! read-side optimisation. See [`crate::kv`] module docs.

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use bigdecimal::BigDecimal;
use chrono::{DateTime, TimeZone, Utc};
use eth_tools_core::events::{
    MetadataSet, Transfer, METADATA_SET_TOPIC, TRANSFER_TOPIC,
};
use eth_tools_db::{agents_history, cursors};
use eth_tools_rpc::{confirmed_head, RotatingProvider, MAX_LOGS_PER_CALL, REORG_SAFETY_BLOCKS};
use sqlx::{PgPool, Postgres, Transaction};
use vercel_runtime::Error;

use crate::kv::{agent_wallet_key, KvInvalidator};
use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "wallet_rotation_watcher";

/// Magic key ERC-8004 uses for the agent's hot wallet via `MetadataSet`.
const METADATA_AGENT_WALLET_KEY: &str = "agentWallet";

/// Worker suffix appended to the cursor key so W6 doesn't share state
/// with W1 (which scrapes the same topics on a different cadence).
const W6_CURSOR_SUFFIX: &str = ":w6";

/// The two event topics W6 cares about.
const WATCHED_TOPICS: &[(&str, B256)] = &[
    ("Transfer", TRANSFER_TOPIC),
    ("MetadataSet", METADATA_SET_TOPIC),
];

/// Public entrypoint called from `api/cron/wallet_rotation_watcher.rs`.
///
/// `kv` is an injectable invalidator so tests can use [`crate::kv::CountingKv`]
/// without spinning up Redis. Production passes the result of
/// [`crate::kv::from_env_or_noop`].
pub async fn run(
    ctx: &WorkerContext,
    chain_id: u64,
    kv: &dyn KvInvalidator,
) -> Result<WorkerSummary, Error> {
    let chain = eth_tools_core::chains::by_id(chain_id)
        .ok_or_else(|| Error::from(format!("unknown chain_id: {chain_id}")))?;
    let identity_registry = Address::from(chain.identity_registry);

    let head = ctx
        .rpc
        .get_block_number()
        .await
        .map_err(|e| Error::from(format!("get_block_number failed: {e}")))?;
    let target = confirmed_head(head, REORG_SAFETY_BLOCKS);
    if target == 0 {
        return Ok(WorkerSummary::ok(WORKER_NAME));
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut invalidated: u64 = 0;

    for (event_name, topic0) in WATCHED_TOPICS {
        let (rows_in, rows_out, evicted) = scrape_and_apply(
            ctx,
            kv,
            chain.chain_id as i64,
            identity_registry,
            *topic0,
            event_name,
            target,
        )
        .await?;
        total_in = total_in.saturating_add(rows_in);
        total_out = total_out.saturating_add(rows_out);
        invalidated = invalidated.saturating_add(evicted);
    }

    tracing::info!(
        worker = WORKER_NAME,
        chain_id,
        logs_seen = total_in,
        agents_touched = total_out,
        cache_keys_evicted = invalidated,
        "wallet_rotation_watcher tick complete"
    );

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

/// Page through the W6 cursor for one topic, apply each log inside its
/// own transaction, and fire a best-effort cache invalidation for every
/// affected agent. Returns `(logs_seen, rows_written, cache_evictions)`.
async fn scrape_and_apply(
    ctx: &WorkerContext,
    kv: &dyn KvInvalidator,
    chain_id: i64,
    contract: Address,
    topic0: B256,
    event_name: &str,
    target: u64,
) -> Result<(u64, u64, u64), Error> {
    let contract_bytes: [u8; 20] = contract.into_array();
    let topic_bytes: [u8; 32] = topic0.into();
    let cursor_key = format!(
        "{}{}",
        cursors::cursor_key(chain_id, &contract_bytes, &topic_bytes),
        W6_CURSOR_SUFFIX
    );

    let from_block = {
        let mut conn = ctx
            .pool
            .acquire()
            .await
            .map_err(|e| Error::from(format!("pool acquire failed: {e}")))?;
        cursors::get(&mut conn, &cursor_key)
            .await
            .map_err(|e| Error::from(format!("cursors::get failed: {e}")))?
            .map(|c| (c.last_block as u64).saturating_add(1))
            .unwrap_or(0)
            .max(chain_genesis(chain_id))
    };

    if from_block > target {
        return Ok((0, 0, 0));
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut total_invalidated: u64 = 0;
    let mut cursor_pos = from_block;
    while cursor_pos <= target {
        let page_end = cursor_pos.saturating_add(MAX_LOGS_PER_CALL - 1).min(target);

        let logs = fetch_page(&ctx.rpc, contract, topic0, cursor_pos, page_end).await?;
        total_in = total_in.saturating_add(logs.len() as u64);

        let (rows_written, evictions) = if ctx.dryrun {
            tracing::info!(
                worker = WORKER_NAME,
                event = event_name,
                from_block = cursor_pos,
                to_block = page_end,
                log_count = logs.len(),
                "dryrun: skipping commit + cache invalidation"
            );
            (0u64, 0u64)
        } else {
            commit_page(ctx, kv, chain_id, event_name, &logs, &cursor_key, page_end).await?
        };
        total_out = total_out.saturating_add(rows_written);
        total_invalidated = total_invalidated.saturating_add(evictions);

        cursor_pos = match page_end.checked_add(1) {
            Some(c) => c,
            None => break,
        };
    }

    Ok((total_in, total_out, total_invalidated))
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

/// Apply one page of logs in a single tx (DB consistency), then fire
/// cache invalidations for each affected agent OUTSIDE the tx (so a
/// slow Redis doesn't hold the DB lock). Returns `(rows_written, evictions)`.
async fn commit_page(
    ctx: &WorkerContext,
    kv: &dyn KvInvalidator,
    chain_id: i64,
    event_name: &str,
    logs: &[Log],
    cursor_key: &str,
    page_end: u64,
) -> Result<(u64, u64), Error> {
    let mut tx = ctx
        .pool
        .begin()
        .await
        .map_err(|e| Error::from(format!("tx begin failed: {e}")))?;

    let mut last_log_index_i32: i32 = 0;
    let mut written: u64 = 0;
    // Collect (chain_id, agent_id_decimal) tuples to invalidate AFTER
    // commit — Redis hiccups must not roll back the DB.
    let mut to_invalidate: Vec<(i64, String)> = Vec::new();

    for log in logs {
        let log_index = log.log_index.unwrap_or(0);
        last_log_index_i32 = log_index.try_into().unwrap_or(i32::MAX);

        match apply_log(&mut tx, chain_id, event_name, log).await {
            Ok(Some(agent_id_dec)) => {
                written = written.saturating_add(1);
                to_invalidate.push((chain_id, agent_id_dec));
            }
            Ok(None) => {} // no-op event (e.g. unknown metadata key)
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

    let mut evicted: u64 = 0;
    for (chain, agent_dec) in to_invalidate {
        let key = agent_wallet_key(chain, &agent_dec);
        match kv.del(&key).await {
            Ok(_) => {
                evicted = evicted.saturating_add(1);
            }
            Err(e) => {
                // Best-effort — log + continue. Worker invariant (DB row
                // updated) already holds.
                tracing::warn!(
                    worker = WORKER_NAME,
                    key = %key,
                    error = %e,
                    "kv invalidation failed; continuing"
                );
            }
        }
    }
    Ok((written, evicted))
}

/// Apply one log. Returns `Ok(Some(agent_id_decimal))` if the agents row
/// was touched and a cache invalidation should fire; `Ok(None)` if the
/// log was a no-op (e.g. mint Transfer, unknown MetadataSet key, agent
/// not yet in `agents`).
async fn apply_log<'c>(
    tx: &mut Transaction<'c, Postgres>,
    chain_id: i64,
    event_name: &str,
    log: &Log,
) -> Result<Option<String>, Error> {
    let block_number = log
        .block_number
        .ok_or_else(|| Error::from("log missing block_number"))?;
    let _ = block_number;
    let log_index = log.log_index.unwrap_or(0) as i32;
    let tx_hash_bytes = log
        .transaction_hash
        .map(|h| h.0.to_vec())
        .unwrap_or_else(|| vec![0u8; 32]);
    let block_ts = block_timestamp_or_now(log);

    match event_name {
        "Transfer" => {
            let decoded = Transfer::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode Transfer failed: {e}")))?;
            if decoded.from == Address::ZERO {
                // Mint — Registered handles initial insert (W1's job).
                return Ok(None);
            }
            let agent_id = u256_to_bigdecimal(decoded.tokenId);
            let from_bytes = decoded.from.0.to_vec();
            let to_bytes = decoded.to.0.to_vec();

            // Spec gotcha #2: clear agent_wallet, rotate owner.
            let result = sqlx::query(
                "UPDATE agents
                    SET owner        = $3,
                        agent_wallet = NULL,
                        updated_at   = $4
                  WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .bind(&to_bytes)
            .bind(block_ts)
            .execute(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents Transfer update failed: {e}")))?;

            if result.rows_affected() == 0 {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    "Transfer for unknown agent — W1 hasn't ingested Registered yet? skipping"
                );
                return Ok(None);
            }

            agents_history::insert_in_tx(
                tx,
                agents_history::HistoryInsert {
                    chain_id,
                    agent_id: &agent_id,
                    changed_at: block_ts,
                    change_kind: agents_history::KIND_OWNER,
                    before_value: Some(serde_json::Value::String(format!(
                        "0x{}",
                        hex_lower(&from_bytes)
                    ))),
                    after_value: Some(serde_json::Value::String(format!(
                        "0x{}",
                        hex_lower(&to_bytes)
                    ))),
                    tx_hash: &tx_hash_bytes,
                    log_index,
                },
            )
            .await
            .map_err(|e| Error::from(format!("agents_history insert failed: {e}")))?;
            Ok(Some(agent_id.to_string()))
        }
        "MetadataSet" => {
            let decoded = MetadataSet::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode MetadataSet failed: {e}")))?;
            if decoded.key != METADATA_AGENT_WALLET_KEY {
                return Ok(None);
            }
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let value_bytes: Vec<u8> = decoded.value.to_vec();

            let new_wallet = if value_bytes.len() == 20 {
                value_bytes.clone()
            } else if value_bytes.len() == 32 {
                value_bytes[12..32].to_vec()
            } else {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    value_len = value_bytes.len(),
                    "MetadataSet(agentWallet) malformed value — skipping"
                );
                return Ok(None);
            };

            let before_wallet: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT agent_wallet FROM agents WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents pre-read failed: {e}")))?
            .flatten();

            let result = sqlx::query(
                "UPDATE agents
                    SET agent_wallet = $3,
                        updated_at   = $4
                  WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .bind(&new_wallet)
            .bind(block_ts)
            .execute(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents wallet update failed: {e}")))?;

            if result.rows_affected() == 0 {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    "MetadataSet for unknown agent — skipping"
                );
                return Ok(None);
            }

            agents_history::insert_in_tx(
                tx,
                agents_history::HistoryInsert {
                    chain_id,
                    agent_id: &agent_id,
                    changed_at: block_ts,
                    change_kind: agents_history::KIND_WALLET,
                    before_value: before_wallet
                        .map(|w| serde_json::Value::String(format!("0x{}", hex_lower(&w)))),
                    after_value: Some(serde_json::Value::String(format!(
                        "0x{}",
                        hex_lower(&new_wallet)
                    ))),
                    tx_hash: &tx_hash_bytes,
                    log_index,
                },
            )
            .await
            .map_err(|e| Error::from(format!("agents_history insert failed: {e}")))?;
            Ok(Some(agent_id.to_string()))
        }
        _ => {
            tracing::error!(worker = WORKER_NAME, event = event_name, "unknown event");
            Ok(None)
        }
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

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn chain_genesis(chain_id: i64) -> u64 {
    eth_tools_core::chains::by_id(chain_id as u64)
        .map(|c| c.genesis_block)
        .unwrap_or(0)
}

/// Re-exported for the cron handler so it doesn't depend on PgPool
/// directly. Kept private to the worker for now — bridge through
/// [`crate::wallet_rotation_watcher::run`].
#[allow(dead_code)]
fn _pool_marker(_p: &PgPool) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kv::CountingKv;

    #[test]
    fn cursor_key_namespace_disjoint_from_w1() {
        // W1's key has no suffix; W6's has ":w6". They must differ for
        // the same (chain, contract, topic).
        let contract = [0xABu8; 20];
        let topic = [0xCDu8; 32];
        let w1 = cursors::cursor_key(8453, &contract, &topic);
        let w6 = format!("{w1}{W6_CURSOR_SUFFIX}");
        assert_ne!(w1, w6);
        assert!(w6.ends_with(":w6"));
    }

    #[test]
    fn watched_topics_are_transfer_and_metadata() {
        let names: Vec<&str> = WATCHED_TOPICS.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"Transfer"));
        assert!(names.contains(&"MetadataSet"));
        assert_eq!(names.len(), 2);
    }

    #[tokio::test]
    async fn counting_kv_invalidator_is_usable() {
        // Smoke: the test seam works as a `&dyn KvInvalidator`.
        let kv = CountingKv::new();
        let bound: &dyn KvInvalidator = &kv;
        bound.del("agent_wallet:8453:42").await.unwrap();
        assert_eq!(kv.keys().await, vec!["agent_wallet:8453:42".to_string()]);
    }

    #[test]
    fn u256_roundtrips() {
        assert_eq!(u256_to_bigdecimal(U256::MAX).to_string(), U256::MAX.to_string());
        assert_eq!(u256_to_bigdecimal(U256::ZERO).to_string(), "0");
    }
}
