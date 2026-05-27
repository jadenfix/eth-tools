//! Worker W1 — registry scraper (plan §3 row W1).
//!
//! Reads the ERC-8004 Identity registry events on a 2-minute cron and upserts
//! agent rows into Postgres. Four event topics are scraped per tick:
//!
//!   1. [`Registered`]      → INSERT INTO agents
//!   2. [`URIUpdated`]      → UPDATE agents SET agent_uri
//!   3. [`MetadataSet`]     → UPDATE agents SET agent_wallet (when key == "agentWallet")
//!   4. ERC-721 [`Transfer`] → UPDATE agents SET owner = to, agent_wallet = NULL
//!
//! ## Reorg safety
//!
//! Workers target `head - REORG_SAFETY_BLOCKS` (12 blocks ≈ 24 s on Base —
//! Coinbase's published finality recommendation). Anything past the safe head
//! is dropped on this tick and re-scanned next time the cron fires.
//!
//! ## Cursor + idempotency
//!
//! One cursor per `(chain_id, contract, topic0)` triple — same shape used by
//! the rest of the scrapers. On each page, the event upsert AND the cursor
//! advance happen in a **single transaction**: a crashed worker re-reads the
//! same cursor on the next tick and replays the unprocessed prefix; an
//! `ON CONFLICT` upsert keeps replay idempotent.
//!
//! ## Pagination
//!
//! `eth_getLogs` is paged in [`MAX_LOGS_PER_CALL`] = 2 000-block windows —
//! Alchemy's hard limit, safely under every other provider's. The rotator
//! handles failover between providers per call; we never need to special-case
//! a "got partial range" return.
//!
//! ## What W1 does NOT do
//!
//! - Fetch agent manifests (W2 manifest_fetcher).
//! - Probe endpoints (W3 endpoint_prober).
//! - Compute trust scores (W7 trust_score_recompute).
//! - Invalidate cached wallets (W6 wallet_rotation_watcher uses the same
//!   `Transfer` / `MetadataSet` events but with a different invariant —
//!   they're complementary, not redundant; see plan §3 W6).

use std::sync::Arc;

use alloy::primitives::{Address, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use bigdecimal::BigDecimal;
use chrono::{DateTime, TimeZone, Utc};
use eth_tools_core::events::{
    MetadataSet, Registered, Transfer, URIUpdated, METADATA_SET_TOPIC, REGISTERED_TOPIC,
    TRANSFER_TOPIC, URI_UPDATED_TOPIC,
};
use eth_tools_db::cursors;
use eth_tools_rpc::{confirmed_head, RotatingProvider, MAX_LOGS_PER_CALL, REORG_SAFETY_BLOCKS};
use sqlx::{PgPool, Postgres, Transaction};
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "registry_scraper";

/// Magic key used by ERC-8004 `MetadataSet` to rotate the agent's hot wallet.
/// Any other key is recorded to `agents_history` as `change_kind='metadata'`
/// but does NOT touch the `agents` row.
const METADATA_AGENT_WALLET_KEY: &str = "agentWallet";

/// Spec-stipulated max gap between cursor and target before we log a warning.
/// 50 000 blocks ≈ 28 hours on Base — anything larger means W1 has fallen
/// behind and operators should investigate.
const LAG_WARN_BLOCKS: u64 = 50_000;

/// One bag of the four event topics we scrape this tick. Used internally to
/// drive the per-topic page loop.
const SCRAPED_TOPICS: &[(&str, B256)] = &[
    ("Registered", REGISTERED_TOPIC),
    ("URIUpdated", URI_UPDATED_TOPIC),
    ("MetadataSet", METADATA_SET_TOPIC),
    ("Transfer", TRANSFER_TOPIC),
];

/// Public entrypoint called from `api/cron/registry_scraper.rs`.
///
/// Returns a [`WorkerSummary`] with `rows_in` = total logs decoded across all
/// topics and `rows_out` = total `agents` upserts/updates committed.
pub async fn run(ctx: &WorkerContext, chain_id: u64) -> Result<WorkerSummary, Error> {
    let chain = eth_tools_core::chains::by_id(chain_id)
        .ok_or_else(|| Error::from(format!("unknown chain_id: {chain_id}")))?;
    let identity_registry = Address::from(chain.identity_registry);

    // 1. Head block.
    let head = ctx
        .rpc
        .get_block_number()
        .await
        .map_err(|e| Error::from(format!("get_block_number failed: {e}")))?;

    // 2. Reorg-safe target: head - 12.
    let target = confirmed_head(head, REORG_SAFETY_BLOCKS);
    tracing::info!(
        worker = WORKER_NAME,
        chain_id,
        head,
        target,
        reorg_buffer = REORG_SAFETY_BLOCKS,
        "registry_scraper tick start"
    );

    if target == 0 {
        // Early chain (anvil from block 0) — nothing to do yet.
        return Ok(WorkerSummary::ok(WORKER_NAME));
    }

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;

    for (event_name, topic0) in SCRAPED_TOPICS {
        let (rows_in, rows_out) = scrape_one_topic(
            ctx,
            chain.chain_id as i64,
            identity_registry,
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

/// Page through `eth_getLogs` for a single (chain, contract, topic0) triple,
/// committing each page in its own transaction so a mid-paginate crash leaves
/// a consistent partial-progress cursor.
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

    // Read existing cursor outside the tx — cheap, and lets us bail early if
    // we're already caught up.
    let from_block = {
        let mut conn = ctx
            .pool
            .acquire()
            .await
            .map_err(|e| Error::from(format!("pool acquire failed: {e}")))?;
        let cursor = cursors::get(&mut conn, &cursor_key)
            .await
            .map_err(|e| Error::from(format!("cursors::get({cursor_key}) failed: {e}")))?;
        // Resume at last_block + 1 — log_index dedup happens via the upsert
        // ON CONFLICT clause, no need to thread it into the filter.
        cursor.map(|c| (c.last_block as u64).saturating_add(1)).unwrap_or(0)
    };

    // Use the chain's genesis block as the floor so first-run scrapes don't
    // start at block 0 (Base is at block ≈ 41M; scanning 0..41M is a free
    // way to exhaust Alchemy's monthly quota).
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

    tracing::info!(
        worker = WORKER_NAME,
        event = event_name,
        from_block,
        target,
        lag_blocks = lag,
        "scraping event"
    );

    let mut total_in: u64 = 0;
    let mut total_out: u64 = 0;
    let mut cursor = from_block;
    while cursor <= target {
        // Inclusive 2 000-block page.
        let page_end = cursor
            .saturating_add(MAX_LOGS_PER_CALL - 1)
            .min(target);

        let logs = fetch_page(&ctx.rpc, contract, topic0, cursor, page_end).await?;
        total_in = total_in.saturating_add(logs.len() as u64);

        // Even an empty page advances the cursor — otherwise the next tick
        // re-scans the same gap forever.
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
            commit_page(
                &ctx.pool,
                chain_id,
                event_name,
                &logs,
                &cursor_key,
                page_end,
            )
            .await?
        };
        total_out = total_out.saturating_add(written);

        cursor = match page_end.checked_add(1) {
            Some(c) => c,
            None => break,
        };
    }

    Ok((total_in, total_out))
}

/// Fetch one 2 000-block page of logs via the rotator.
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

/// Apply one page of logs + advance the cursor in a single transaction. On
/// any error rolls back so the cursor doesn't advance past unprocessed logs.
/// Returns the count of `agents`-row upserts committed (which == rows_out).
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

    // Find the last log's log_index within the page so the cursor record is
    // exact. Default to 0 for an empty page — the cursor still advances by
    // block.
    let mut last_log_index_i32: i32 = 0;
    let mut written: u64 = 0;

    for log in logs {
        let log_index = log.log_index.unwrap_or(0);
        last_log_index_i32 = log_index.try_into().unwrap_or(i32::MAX);

        match apply_log(&mut tx, chain_id, event_name, log).await {
            Ok(applied) => {
                if applied {
                    written = written.saturating_add(1);
                }
            }
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
                // Explicit rollback for clarity; Drop would also do it.
                let _ = tx.rollback().await;
                return Err(e);
            }
        }
    }

    // Cursor: advance to page_end regardless of log count so we don't re-scan
    // empty ranges. last_log_index is the last we processed (or 0 for empty).
    cursors::advance(&mut tx, cursor_key, page_end as i64, last_log_index_i32)
        .await
        .map_err(|e| Error::from(format!("cursors::advance failed: {e}")))?;

    tx.commit()
        .await
        .map_err(|e| Error::from(format!("tx commit failed: {e}")))?;

    Ok(written)
}

/// Dispatch one log to the correct UPSERT/UPDATE based on its event signature.
/// Returns `Ok(true)` if it touched `agents`, `Ok(false)` if it was a no-op
/// (e.g. `MetadataSet` with a key we don't track yet, or a Transfer for an
/// agent we've never seen).
async fn apply_log(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    event_name: &str,
    log: &Log,
) -> Result<bool, Error> {
    // block_number is required — if the provider didn't return it, we can't
    // commit a deterministic agents_history row for the change. Fail the page
    // so the cursor doesn't advance past unprocessed logs.
    let _block_number = log
        .block_number
        .ok_or_else(|| Error::from("log missing block_number"))?;
    let log_index = log.log_index.unwrap_or(0) as i32;
    let tx_hash_bytes = log
        .transaction_hash
        .map(|h| h.0.to_vec())
        .unwrap_or_else(|| vec![0u8; 32]);
    let block_ts = block_timestamp_or_now(log);

    match event_name {
        "Registered" => {
            let decoded = Registered::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode Registered failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let owner_bytes = decoded.owner.0.to_vec();
            let agent_uri = decoded.agentUri.clone();

            // INSERT ... ON CONFLICT DO UPDATE — restartable + idempotent.
            // On re-scrape of an already-known agent: refresh owner+uri (a
            // reorg could re-emit Registered with different values; the
            // canonical state is the *last* event we saw).
            sqlx::query(
                "INSERT INTO agents (chain_id, agent_id, owner, agent_uri, agent_wallet,
                                     registered_at, updated_at)
                 VALUES ($1, $2, $3, $4, NULL, $5, $5)
                 ON CONFLICT (chain_id, agent_id) DO UPDATE
                   SET owner      = EXCLUDED.owner,
                       agent_uri  = EXCLUDED.agent_uri,
                       updated_at = $5",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .bind(&owner_bytes)
            .bind(&agent_uri)
            .bind(block_ts)
            .execute(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents upsert failed: {e}")))?;

            insert_history(
                tx,
                chain_id,
                &agent_id,
                "metadata",
                None,
                Some(serde_json::json!({
                    "owner": format!("0x{}", hex_lower(&owner_bytes)),
                    "agent_uri": agent_uri,
                })),
                &tx_hash_bytes,
                log_index,
                block_ts,
            )
            .await?;
            Ok(true)
        }
        "URIUpdated" => {
            let decoded = URIUpdated::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode URIUpdated failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let new_uri = decoded.newUri.clone();

            // Capture the prior URI for the history row before we overwrite.
            let before_uri: Option<String> = sqlx::query_scalar(
                "SELECT agent_uri FROM agents WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents pre-read failed: {e}")))?
            .flatten();

            let result = sqlx::query(
                "UPDATE agents
                    SET agent_uri  = $3,
                        updated_at = $4
                  WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .bind(&new_uri)
            .bind(block_ts)
            .execute(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents update agent_uri failed: {e}")))?;

            if result.rows_affected() == 0 {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    "URIUpdated for unknown agent — Registered event lost? skipping"
                );
                return Ok(false);
            }

            insert_history(
                tx,
                chain_id,
                &agent_id,
                "uri",
                before_uri.map(serde_json::Value::String),
                Some(serde_json::Value::String(new_uri)),
                &tx_hash_bytes,
                log_index,
                block_ts,
            )
            .await?;
            Ok(true)
        }
        "MetadataSet" => {
            let decoded = MetadataSet::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode MetadataSet failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.agentId);
            let key = decoded.key.clone();
            let value_bytes: Vec<u8> = decoded.value.to_vec();

            if key != METADATA_AGENT_WALLET_KEY {
                // Record other metadata keys for future audit but don't touch
                // the agents row — only `agentWallet` has a typed column.
                insert_history(
                    tx,
                    chain_id,
                    &agent_id,
                    "metadata",
                    None,
                    Some(serde_json::json!({
                        "key": key,
                        "value_hex": format!("0x{}", hex_lower(&value_bytes)),
                    })),
                    &tx_hash_bytes,
                    log_index,
                    block_ts,
                )
                .await?;
                return Ok(false);
            }

            // agentWallet value MUST be a 20-byte address per ERC-8004. The
            // spec allows trailing/leading padding from `bytes`-typed setters;
            // we accept exactly 20 bytes or refuse to update (a malformed
            // value would otherwise overwrite a known-good wallet with junk).
            let new_wallet = if value_bytes.len() == 20 {
                value_bytes.clone()
            } else if value_bytes.len() == 32 {
                // 32-byte abi-encoded address: 12 zero bytes + 20 address bytes.
                value_bytes[12..32].to_vec()
            } else {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    value_len = value_bytes.len(),
                    "MetadataSet(agentWallet) with non-address-shaped value — skipping"
                );
                return Ok(false);
            };

            let before_wallet: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT agent_wallet FROM agents WHERE chain_id = $1 AND agent_id = $2",
            )
            .bind(chain_id)
            .bind(&agent_id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| Error::from(format!("agents pre-read agent_wallet failed: {e}")))?
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
            .map_err(|e| Error::from(format!("agents update agent_wallet failed: {e}")))?;

            if result.rows_affected() == 0 {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    "MetadataSet(agentWallet) for unknown agent — skipping"
                );
                return Ok(false);
            }

            insert_history(
                tx,
                chain_id,
                &agent_id,
                "wallet",
                before_wallet
                    .map(|w| serde_json::Value::String(format!("0x{}", hex_lower(&w)))),
                Some(serde_json::Value::String(format!("0x{}", hex_lower(&new_wallet)))),
                &tx_hash_bytes,
                log_index,
                block_ts,
            )
            .await?;
            Ok(true)
        }
        "Transfer" => {
            let decoded = Transfer::decode_log(&log.inner)
                .map_err(|e| Error::from(format!("decode Transfer failed: {e}")))?;
            let agent_id = u256_to_bigdecimal(decoded.tokenId);
            let from_bytes = decoded.from.0.to_vec();
            let to_bytes = decoded.to.0.to_vec();

            // Ignore mints — Registered handles the initial row insert. ERC-721
            // transfers from the zero address are mints; trying to UPDATE the
            // row here would race with Registered for the same tx and bounce
            // (0 rows_affected, since Registered runs in the same block).
            //
            // Tracking by zero from-address is sufficient — the registry
            // only mints in Register().
            if decoded.from == Address::ZERO {
                return Ok(false);
            }

            // Spec gotcha #2: wallet MUST be cleared on transfer. Otherwise a
            // poisoned cache would let the previous owner keep receiving
            // payments. See plan §3 W6 + W1 row.
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
            .map_err(|e| Error::from(format!("agents update on Transfer failed: {e}")))?;

            if result.rows_affected() == 0 {
                tracing::warn!(
                    worker = WORKER_NAME,
                    chain_id,
                    agent_id = %agent_id,
                    "Transfer for unknown agent — out-of-order events? skipping"
                );
                return Ok(false);
            }

            insert_history(
                tx,
                chain_id,
                &agent_id,
                "owner",
                Some(serde_json::Value::String(format!("0x{}", hex_lower(&from_bytes)))),
                Some(serde_json::Value::String(format!("0x{}", hex_lower(&to_bytes)))),
                &tx_hash_bytes,
                log_index,
                block_ts,
            )
            .await?;
            Ok(true)
        }
        _ => {
            tracing::error!(worker = WORKER_NAME, event = event_name, "unknown event");
            Ok(false)
        }
    }
}

/// Audit-row writer for `agents_history`. The argument count is intrinsic —
/// every history row needs (chain, agent, when, kind, before, after, tx, idx)
/// to be useful for /dashboard/agents/:id timeline rendering. Grouping into a
/// struct would just hide the args behind one more layer.
#[allow(clippy::too_many_arguments)]
async fn insert_history(
    tx: &mut Transaction<'_, Postgres>,
    chain_id: i64,
    agent_id: &BigDecimal,
    change_kind: &'static str,
    before_value: Option<serde_json::Value>,
    after_value: Option<serde_json::Value>,
    tx_hash: &[u8],
    log_index: i32,
    changed_at: DateTime<Utc>,
) -> Result<(), Error> {
    sqlx::query(
        "INSERT INTO agents_history
            (chain_id, agent_id, changed_at, change_kind, before_value, after_value,
             tx_hash, log_index)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(chain_id)
    .bind(agent_id)
    .bind(changed_at)
    .bind(change_kind)
    .bind(before_value)
    .bind(after_value)
    .bind(tx_hash)
    .bind(log_index)
    .execute(&mut **tx)
    .await
    .map_err(|e| Error::from(format!("agents_history insert failed: {e}")))?;
    Ok(())
}

/// `block_timestamp` is provided by Alchemy and most modern Base RPCs but is
/// NOT guaranteed by `eth_getLogs` spec. Falling back to `NOW()` keeps the
/// scraper resilient on legacy providers; the `agents_history.changed_at`
/// column captures the *observed* time anyway.
fn block_timestamp_or_now(log: &Log) -> DateTime<Utc> {
    log.block_timestamp
        .and_then(|ts| Utc.timestamp_opt(ts as i64, 0).single())
        .unwrap_or_else(Utc::now)
}

/// Convert a 256-bit agent id into a `NUMERIC(78, 0)`-compatible decimal.
/// `U256::to_string()` already produces a decimal representation.
fn u256_to_bigdecimal(value: U256) -> BigDecimal {
    // U256 -> decimal string -> BigDecimal. This roundtrips losslessly for
    // any uint256 because BigDecimal is arbitrary precision.
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

/// Per-chain `from_block` floor on first run. Pulled from
/// `eth_tools_core::chains` so the scraper, dashboard, and CLI all agree.
fn chain_genesis(chain_id: i64) -> u64 {
    eth_tools_core::chains::by_id(chain_id as u64)
        .map(|c| c.genesis_block)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u256_roundtrips_to_bigdecimal() {
        // Largest uint256 — must survive without overflow.
        let max = U256::MAX;
        let bd = u256_to_bigdecimal(max);
        // BigDecimal stringification drops trailing zeros only for scaled
        // values; integers round-trip.
        assert_eq!(bd.to_string(), max.to_string());
    }

    #[test]
    fn u256_zero_roundtrips() {
        assert_eq!(u256_to_bigdecimal(U256::ZERO).to_string(), "0");
    }

    #[test]
    fn hex_lower_no_uppercase() {
        let bytes = [0xDEu8, 0xAD, 0xBE, 0xEF];
        let s = hex_lower(&bytes);
        assert_eq!(s, "deadbeef");
        assert!(!s.chars().any(|c| c.is_ascii_uppercase()));
    }

    #[test]
    fn chain_genesis_uses_core_constant() {
        // Should match the Base mainnet genesis block in core::chains.
        assert_eq!(chain_genesis(8453), 41_663_783);
        // Unknown chain → 0.
        assert_eq!(chain_genesis(99_999), 0);
    }

    #[test]
    fn block_timestamp_or_now_prefers_log_ts() {
        let log = Log {
            block_timestamp: Some(1_700_000_000),
            ..Log::default()
        };
        let ts = block_timestamp_or_now(&log);
        assert_eq!(ts.timestamp(), 1_700_000_000);
    }

    #[test]
    fn block_timestamp_or_now_falls_back() {
        let log = Log::default();
        let ts = block_timestamp_or_now(&log);
        let now = Utc::now();
        // Within 5 seconds of "now" — any later and the test machine is in
        // bad shape.
        assert!((now.timestamp() - ts.timestamp()).abs() <= 5);
    }

    /// All four topics in our `SCRAPED_TOPICS` table are non-zero and
    /// distinct — guards against an accidental import that swaps two
    /// constants.
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
}
