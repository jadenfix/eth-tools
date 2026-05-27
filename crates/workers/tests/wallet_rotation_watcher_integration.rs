//! Integration test for W6 — `wallet_rotation_watcher`.
//!
//! Builds synthetic ERC-721 Transfer + MetadataSet logs, feeds them through
//! a mock `RpcProvider`, and asserts:
//!   - On Transfer: `agent_wallet` is cleared, `owner` rotated,
//!     agents_history row written, CountingKv saw the eviction.
//!   - On MetadataSet(agentWallet): `agent_wallet` set, CountingKv saw it.
//!   - Cache invalidations fire AFTER the DB commit (the test's KV records
//!     the key shape per `agent_wallet:{chain}:{agent_id}`).
//!
//! Skipped automatically if Docker isn't available.

#![cfg(test)]

use std::sync::Arc;

use alloy::primitives::{Address, LogData, B256, U256};
use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use eth_tools_core::events::{MetadataSet, Transfer};
use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider};
use eth_tools_workers::kv::CountingKv;
use eth_tools_workers::{wallet_rotation_watcher, WorkerContext};
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use tokio::sync::Mutex;

const IDENTITY_REGISTRY_HEX: &str = "8004A169FB4a3325136EB29fA0ceB6D2e539a432";

async fn boot() -> Option<(impl std::fmt::Debug, eth_tools_db::Pool)> {
    let container = match Postgres::default().with_tag("16-alpine").start().await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[skip] Docker unavailable: {e}");
            return None;
        }
    };
    let host = container.get_host().await.ok()?;
    let port = container.get_host_port_ipv4(5432).await.ok()?;
    let url = format!("postgres://postgres:postgres@{host}:{port}/postgres");
    let pool = eth_tools_db::connect(&url).await.expect("connect");
    eth_tools_db::migrate(&pool).await.expect("migrate");
    Some((container, pool))
}

async fn seed_chain(pool: &eth_tools_db::Pool) {
    sqlx::query(
        "INSERT INTO chains (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
         VALUES (8453, 'base', '\\x00'::bytea, '\\x00'::bytea, '\\x00'::bytea, 'http://stub', false)
         ON CONFLICT DO NOTHING",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_agent(pool: &eth_tools_db::Pool, agent_id: i64, owner: [u8; 20], wallet: Option<[u8; 20]>) {
    let id = BigDecimal::from(agent_id);
    let wallet_vec: Option<Vec<u8>> = wallet.map(|w| w.to_vec());
    sqlx::query(
        "INSERT INTO agents (chain_id, agent_id, owner, agent_uri, agent_wallet, registered_at, updated_at)
         VALUES (8453, $1, $2, 'http://x', $3, NOW(), NOW())
         ON CONFLICT DO NOTHING",
    )
    .bind(&id)
    .bind(owner.to_vec())
    .bind(wallet_vec)
    .execute(pool)
    .await
    .unwrap();
}

/// Mock RPC: returns a fixed head block, and pulls logs from a queue
/// populated by the test. Each call drains the front of the queue.
struct ScriptedRpc {
    head: u64,
    queue: Mutex<Vec<alloy::rpc::types::Log>>,
}

#[async_trait]
impl RpcProvider for ScriptedRpc {
    fn name(&self) -> &str {
        "scripted"
    }
    async fn get_block_number(&self) -> Result<u64, RpcError> {
        Ok(self.head)
    }
    async fn get_logs(
        &self,
        filter: &alloy::rpc::types::Filter,
    ) -> Result<Vec<alloy::rpc::types::Log>, RpcError> {
        // Filter the queue by topic so the two-topic loop returns the
        // right subset per call. The worker queries one topic at a time
        // via `Filter::event_signature`, which populates `filter.topics[0]`.
        let want_topic: Option<B256> = filter.topics[0].iter().next().copied();
        let mut q = self.queue.lock().await;
        let mut out = Vec::new();
        q.retain(|log| {
            let log_topic = log.inner.topics().first().copied();
            match (want_topic, log_topic) {
                (Some(w), Some(l)) if w == l => {
                    out.push(log.clone());
                    false // remove from queue once delivered
                }
                _ => true, // keep for a later topic-specific call
            }
        });
        Ok(out)
    }
    async fn get_balance(&self, _: Address) -> Result<U256, RpcError> {
        Ok(U256::ZERO)
    }
}

fn parse_identity_addr() -> Address {
    let mut bytes = [0u8; 20];
    for i in 0..20 {
        bytes[i] = u8::from_str_radix(&IDENTITY_REGISTRY_HEX[i * 2..i * 2 + 2], 16).unwrap();
    }
    Address::from(bytes)
}

fn make_log(topics: Vec<B256>, data: Vec<u8>, block_number: u64, log_index: u64) -> alloy::rpc::types::Log {
    let mut log = alloy::rpc::types::Log::default();
    log.inner.address = parse_identity_addr();
    log.inner.data = LogData::new_unchecked(topics, data.into());
    log.block_number = Some(block_number);
    log.log_index = Some(log_index);
    log.transaction_hash = Some(B256::repeat_byte(0xAB));
    log
}

#[tokio::test]
async fn w6_transfer_clears_wallet_and_invalidates_cache() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;

    // Pre-seed an agent owned by addr 0x01 with wallet 0xAA.
    seed_agent(&pool, 42, [0x01; 20], Some([0xAA; 20])).await;

    // Build a Transfer(from=0x01, to=0x02, tokenId=42) log.
    let from = Address::from([0x01u8; 20]);
    let to = Address::from([0x02u8; 20]);
    let token_id = U256::from(42u64);
    let event = Transfer {
        from,
        to,
        tokenId: token_id,
    };
    let encoded = event.encode_log_data();
    let log = make_log(
        encoded.topics().to_vec(),
        encoded.data.to_vec(),
        // Past the chain's genesis block (Base mainnet 41_663_783) and
        // well inside `head - REORG_SAFETY_BLOCKS` so the worker picks it up.
        50_000_000,
        0,
    );

    let rpc = Arc::new(RotatingProvider::new(vec![Arc::new(ScriptedRpc {
        head: 60_000_000,
        queue: Mutex::new(vec![log]),
    })]));
    let ctx = WorkerContext {
        pool: pool.clone(),
        rpc,
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    };

    let kv = CountingKv::new();
    let summary = wallet_rotation_watcher::run(&ctx, 8453, &kv).await.expect("run");
    assert!(summary.ok);
    assert!(summary.rows_in >= 1, "should have seen at least 1 log");

    // Wallet cleared, owner rotated.
    let row: (Vec<u8>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT owner, agent_wallet FROM agents WHERE chain_id = 8453 AND agent_id = 42",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.0, vec![0x02u8; 20], "owner rotated to 'to'");
    assert!(row.1.is_none(), "agent_wallet must be cleared on Transfer");

    // agents_history row written.
    let hist_count: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM agents_history
          WHERE chain_id = 8453 AND agent_id = 42 AND change_kind = 'owner'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hist_count.0, 1, "exactly one owner-change history row");

    // KV invalidation fired with the canonical key.
    let keys = kv.keys().await;
    assert_eq!(keys, vec!["agent_wallet:8453:42".to_string()]);
}

#[tokio::test]
async fn w6_metadata_set_updates_wallet_and_invalidates() {
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;

    seed_agent(&pool, 99, [0x01; 20], None).await;

    let agent_id = U256::from(99u64);
    // Pad the 20-byte target address to 32 bytes (zero-prefix), which the
    // worker accepts and slices.
    let mut padded = vec![0u8; 12];
    padded.extend_from_slice(&[0xBBu8; 20]);
    let event = MetadataSet {
        agentId: agent_id,
        key: "agentWallet".to_string(),
        value: padded.into(),
    };
    let encoded = event.encode_log_data();
    let log = make_log(encoded.topics().to_vec(), encoded.data.to_vec(), 50_000_000, 0);

    let rpc = Arc::new(RotatingProvider::new(vec![Arc::new(ScriptedRpc {
        head: 60_000_000,
        queue: Mutex::new(vec![log]),
    })]));
    let ctx = WorkerContext {
        pool: pool.clone(),
        rpc,
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    };
    let kv = CountingKv::new();
    let _ = wallet_rotation_watcher::run(&ctx, 8453, &kv).await.expect("run");

    let row: (Option<Vec<u8>>,) =
        sqlx::query_as("SELECT agent_wallet FROM agents WHERE agent_id = 99")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(row.0.unwrap(), vec![0xBBu8; 20]);

    let hist: (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM agents_history WHERE agent_id = 99 AND change_kind = 'wallet'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(hist.0, 1);

    let keys = kv.keys().await;
    assert!(keys.contains(&"agent_wallet:8453:99".to_string()));
}

#[tokio::test]
async fn w6_ignores_mint_transfers() {
    // ERC-721 mints (from=0x0) are handled by W1; W6 must skip them.
    let Some((_c, pool)) = boot().await else {
        return;
    };
    seed_chain(&pool).await;

    let event = Transfer {
        from: Address::ZERO,
        to: Address::from([0x05u8; 20]),
        tokenId: U256::from(7u64),
    };
    let encoded = event.encode_log_data();
    let log = make_log(encoded.topics().to_vec(), encoded.data.to_vec(), 50_000_000, 0);

    let rpc = Arc::new(RotatingProvider::new(vec![Arc::new(ScriptedRpc {
        head: 60_000_000,
        queue: Mutex::new(vec![log]),
    })]));
    let ctx = WorkerContext {
        pool: pool.clone(),
        rpc,
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    };
    let kv = CountingKv::new();
    let summary = wallet_rotation_watcher::run(&ctx, 8453, &kv).await.expect("run");
    assert!(summary.ok);
    // No history row, no cache invalidation.
    let hist: (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM agents_history")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(hist.0, 0);
    assert!(kv.keys().await.is_empty(), "mint must not trigger invalidation");
}
