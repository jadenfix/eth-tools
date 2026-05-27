//! End-to-end test for Worker W1 (registry_scraper).
//!
//! Boots a real Postgres via testcontainers, runs all migrations, then
//! exercises [`eth_tools_workers::registry_scraper::run`] against a
//! `MockProvider` that synthesises the four scraped event types from
//! `alloy::sol!` bindings. The provider replays a stable head block so we
//! can deterministically test reorg safety.
//!
//! ## Why one big `#[tokio::test]`?
//!
//! `context::deps()` is a `OnceCell<WorkerDeps>` — multiple parallel tests
//! in the same binary would race on init. Mirrors the pattern in
//! `serve_integration.rs`. Sub-cases below are sequential and each gets a
//! fresh schema (we truncate the relevant tables between cases).
//!
//! ## What this test covers (mapped to the task spec)
//!
//! 1. **Decode + upsert path** for all four events:
//!    - `Registered` → INSERT agents row + history(metadata)
//!    - `URIUpdated` → UPDATE agents.agent_uri + history(uri) with before/after
//!    - `MetadataSet("agentWallet", …)` → UPDATE agents.agent_wallet + history(wallet)
//!    - `Transfer(from, to, id)` (non-mint) → UPDATE owner, NULL wallet, history(owner)
//! 2. **Idempotency**: rerunning the same range produces zero additional
//!    agents-table rows AND zero new history rows (cursor was advanced past
//!    the entire range — second run sees empty filter result).
//! 3. **Reorg safety**: logs at blocks past `head - 12` are NOT processed on
//!    the first tick; bumping head_block past them lets the second tick pick
//!    them up.
//! 4. **Spec gotcha #2** (wallet cleared on transfer): after a `Transfer` the
//!    `agent_wallet` column is NULL even if `MetadataSet(agentWallet, …)` had
//!    just populated it.

#![cfg(test)]

use std::sync::{Arc, Mutex};

use alloy::primitives::{Address, Bytes, FixedBytes, B256, U256};
use alloy::rpc::types::{Filter, Log};
use alloy::sol_types::SolEvent;
use async_trait::async_trait;
use bigdecimal::BigDecimal;
use eth_tools_core::events::{MetadataSet, Registered, Transfer, URIUpdated};
use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider};
use eth_tools_workers::context::{install_for_test, WorkerDeps};
use eth_tools_workers::{registry_scraper, WorkerContext};
use std::str::FromStr;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

const CHAIN_ID: u64 = 8453;
// Identity registry address from `eth_tools_core::chains` — must match what
// the scraper builds its filter against.
const IDENTITY_REGISTRY_HEX: &str = "8004A169FB4a3325136EB29fA0ceB6D2e539a432";
// Far enough above Base genesis (41_663_783) to avoid the chain_genesis
// floor lifting our cursor past the synthetic logs.
const BASE_BLOCK: u64 = 50_000_000;

/// Test-only RPC provider that returns a controllable head + a fixed Vec of
/// pre-built logs filtered by the incoming `Filter`.
struct MockProvider {
    head: Mutex<u64>,
    logs: Mutex<Vec<Log>>,
    get_logs_calls: Mutex<u32>,
}

impl MockProvider {
    fn new(head: u64) -> Self {
        Self {
            head: Mutex::new(head),
            logs: Mutex::new(Vec::new()),
            get_logs_calls: Mutex::new(0),
        }
    }
    fn set_head(&self, head: u64) {
        *self.head.lock().unwrap() = head;
    }
    fn push_log(&self, log: Log) {
        self.logs.lock().unwrap().push(log);
    }
    fn get_logs_call_count(&self) -> u32 {
        *self.get_logs_calls.lock().unwrap()
    }
}

#[async_trait]
impl RpcProvider for MockProvider {
    fn name(&self) -> &str {
        "mock"
    }
    async fn get_block_number(&self) -> Result<u64, RpcError> {
        Ok(*self.head.lock().unwrap())
    }
    async fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, RpcError> {
        *self.get_logs_calls.lock().unwrap() += 1;

        // Filter by address (first address bound) + topic0 + block range.
        // Filter.address/topic0/from_block/to_block live in `FilterSet`/
        // `FilterBlockOption` types — we read what we need by checking which
        // logs match.
        let from_block = filter
            .get_from_block()
            .expect("test filter must set from_block");
        let to_block = filter
            .get_to_block()
            .expect("test filter must set to_block");

        // Match topic0 if specified; the production scraper always sets it.
        let topic0_filter: Option<B256> = filter
            .topics
            .first()
            .and_then(|t| t.iter().next().copied());

        let logs = self.logs.lock().unwrap();
        let out: Vec<Log> = logs
            .iter()
            .filter(|l| {
                let bn = l.block_number.unwrap_or(0);
                if bn < from_block || bn > to_block {
                    return false;
                }
                if let Some(want_t0) = topic0_filter {
                    if l.topic0() != Some(&want_t0) {
                        return false;
                    }
                }
                true
            })
            .cloned()
            .collect();
        Ok(out)
    }
}

/// Build a synthetic `Log<LogData>` for the given event at a given block.
fn make_log<E: SolEvent>(
    event: E,
    address: Address,
    block_number: u64,
    log_index: u64,
    block_timestamp: u64,
) -> Log {
    let log_data = event.encode_log_data();
    let inner = alloy::primitives::Log {
        address,
        data: log_data,
    };
    Log {
        inner,
        block_hash: Some(B256::repeat_byte(0xaa)),
        block_number: Some(block_number),
        block_timestamp: Some(block_timestamp),
        transaction_hash: Some(FixedBytes::repeat_byte(
            // Stable per-(block, log_index) tx hash so dedup-tests can match.
            (block_number as u8).wrapping_add(log_index as u8),
        )),
        transaction_index: Some(0),
        log_index: Some(log_index),
        removed: false,
    }
}

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

fn registry_addr() -> Address {
    Address::from_str(&format!("0x{IDENTITY_REGISTRY_HEX}")).unwrap()
}

async fn count_agents(pool: &eth_tools_db::Pool, chain_id: i64) -> i64 {
    let (c,): (i64,) =
        sqlx::query_as("SELECT COUNT(*)::BIGINT FROM agents WHERE chain_id = $1")
            .bind(chain_id)
            .fetch_one(pool)
            .await
            .expect("count agents");
    c
}

async fn count_history(pool: &eth_tools_db::Pool, chain_id: i64) -> i64 {
    let (c,): (i64,) = sqlx::query_as(
        "SELECT COUNT(*)::BIGINT FROM agents_history WHERE chain_id = $1",
    )
    .bind(chain_id)
    .fetch_one(pool)
    .await
    .expect("count history");
    c
}

async fn truncate_all(pool: &eth_tools_db::Pool) {
    sqlx::query("TRUNCATE TABLE agents_history, agents, cursors RESTART IDENTITY")
        .execute(pool)
        .await
        .expect("truncate");
}

/// Sub-test: each of the 4 events upserts the correct columns.
async fn case_all_four_events(pool: &eth_tools_db::Pool, mock: &Arc<MockProvider>, ctx: &WorkerContext) {
    truncate_all(pool).await;
    mock.logs.lock().unwrap().clear();
    mock.set_head(BASE_BLOCK + 100);

    let addr = registry_addr();
    let agent_id = U256::from(42u64);
    let owner_a = Address::from([0x11u8; 20]);
    let owner_b = Address::from([0x22u8; 20]);
    let wallet = Address::from([0x33u8; 20]);

    // 1) Registered(42, owner_a, "ipfs://manifest1") at block BASE_BLOCK+1
    mock.push_log(make_log(
        Registered {
            agentId: agent_id,
            owner: owner_a,
            agentUri: "ipfs://manifest1".to_string(),
        },
        addr,
        BASE_BLOCK + 1,
        0,
        1_700_000_001,
    ));
    // 2) URIUpdated(42, "ipfs://manifest2") at block BASE_BLOCK+2
    mock.push_log(make_log(
        URIUpdated {
            agentId: agent_id,
            newUri: "ipfs://manifest2".to_string(),
        },
        addr,
        BASE_BLOCK + 2,
        0,
        1_700_000_002,
    ));
    // 3) MetadataSet(42, "agentWallet", wallet bytes) at block BASE_BLOCK+3
    mock.push_log(make_log(
        MetadataSet {
            agentId: agent_id,
            key: "agentWallet".to_string(),
            value: Bytes::from(wallet.0.to_vec()),
        },
        addr,
        BASE_BLOCK + 3,
        0,
        1_700_000_003,
    ));
    // 4) Transfer(owner_a -> owner_b, 42) at block BASE_BLOCK+4
    mock.push_log(make_log(
        Transfer {
            from: owner_a,
            to: owner_b,
            tokenId: agent_id,
        },
        addr,
        BASE_BLOCK + 4,
        0,
        1_700_000_004,
    ));

    // head = BASE_BLOCK+100, target = head - 12 = BASE_BLOCK+88. All 4 logs
    // are well inside the safe range.
    let summary = registry_scraper::run(ctx, CHAIN_ID).await.expect("run");
    assert!(summary.ok);
    // 4 logs decoded, 4 agents-table writes (Registered insert + 3 updates).
    assert_eq!(summary.rows_in, 4, "expected 4 logs decoded");
    assert_eq!(summary.rows_out, 4, "expected 4 agents writes");

    // Inspect the final row.
    let (owner, agent_uri, agent_wallet): (Vec<u8>, Option<String>, Option<Vec<u8>>) =
        sqlx::query_as(
            "SELECT owner, agent_uri, agent_wallet FROM agents
              WHERE chain_id = $1 AND agent_id = $2",
        )
        .bind(CHAIN_ID as i64)
        .bind(BigDecimal::from(42))
        .fetch_one(pool)
        .await
        .expect("fetch final agent");

    // Owner reflects the Transfer (last event).
    assert_eq!(owner, owner_b.0.to_vec(), "owner must be set to transferee");
    // agent_uri reflects URIUpdated.
    assert_eq!(agent_uri.as_deref(), Some("ipfs://manifest2"));
    // Spec gotcha #2: wallet cleared on transfer even though MetadataSet
    // populated it.
    assert!(
        agent_wallet.is_none(),
        "Transfer MUST clear agent_wallet; got {agent_wallet:?}"
    );

    // History rows: 1 metadata (Registered) + 1 uri + 1 wallet + 1 owner.
    let n_history = count_history(pool, CHAIN_ID as i64).await;
    assert_eq!(n_history, 4, "expected 4 history rows; got {n_history}");
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT change_kind FROM agents_history WHERE chain_id = $1 ORDER BY id ASC",
    )
    .bind(CHAIN_ID as i64)
    .fetch_all(pool)
    .await
    .expect("kinds");
    assert_eq!(
        kinds,
        vec!["metadata", "uri", "wallet", "owner"],
        "history rows must record change_kind in event order"
    );
}

/// Sub-test: rerunning the same head/log set must NOT add new rows.
async fn case_idempotency(pool: &eth_tools_db::Pool, mock: &Arc<MockProvider>, ctx: &WorkerContext) {
    // Continuation of `case_all_four_events` — keep its row in place. Run the
    // scraper again with the same head + log set; expect zero new rows.
    let before_agents = count_agents(pool, CHAIN_ID as i64).await;
    let before_history = count_history(pool, CHAIN_ID as i64).await;
    let calls_before = mock.get_logs_call_count();

    let summary = registry_scraper::run(ctx, CHAIN_ID).await.expect("rerun");
    assert!(summary.ok);
    assert_eq!(summary.rows_in, 0, "idempotent rerun must read 0 new logs");
    assert_eq!(summary.rows_out, 0, "idempotent rerun must write 0 rows");

    let after_agents = count_agents(pool, CHAIN_ID as i64).await;
    let after_history = count_history(pool, CHAIN_ID as i64).await;
    assert_eq!(after_agents, before_agents, "agents row count must be stable");
    assert_eq!(
        after_history, before_history,
        "history row count must be stable on rerun"
    );

    // The scraper SHOULD still query the RPC head (to check if there's
    // anything new) but MAY skip get_logs entirely if the cursor is already
    // at target. With cursor == target and no new blocks, no get_logs call.
    assert_eq!(
        mock.get_logs_call_count(),
        calls_before,
        "no new RPC log calls when caught up"
    );
}

/// Sub-test: logs past `head - REORG_SAFETY_BLOCKS` are ignored on this tick
/// but processed on the next tick when head advances.
async fn case_reorg_safety(pool: &eth_tools_db::Pool, mock: &Arc<MockProvider>, ctx: &WorkerContext) {
    truncate_all(pool).await;
    mock.logs.lock().unwrap().clear();

    let addr = registry_addr();
    let agent_id = U256::from(7u64);
    let owner = Address::from([0x44u8; 20]);

    // Place TWO Registered logs: one safely below head-12, one in the danger
    // zone above head-12.
    let head = BASE_BLOCK + 100;
    let safe_block = head - 20; // < head - 12 → safe
    let unsafe_block = head - 5; // > head - 12 → must be skipped on tick 1

    mock.set_head(head);
    mock.push_log(make_log(
        Registered {
            agentId: agent_id,
            owner,
            agentUri: "ipfs://safe".to_string(),
        },
        addr,
        safe_block,
        0,
        1_700_000_010,
    ));
    mock.push_log(make_log(
        Registered {
            agentId: U256::from(8u64),
            owner,
            agentUri: "ipfs://unsafe".to_string(),
        },
        addr,
        unsafe_block,
        0,
        1_700_000_011,
    ));

    // Tick 1: only the safe block should be processed.
    let s1 = registry_scraper::run(ctx, CHAIN_ID).await.expect("tick1");
    assert!(s1.ok);
    assert_eq!(s1.rows_in, 1, "tick1 must skip logs past head-12");
    assert_eq!(count_agents(pool, CHAIN_ID as i64).await, 1);

    // Tick 2: advance head so the previously-unsafe block is now safe.
    mock.set_head(head + 50);
    let s2 = registry_scraper::run(ctx, CHAIN_ID).await.expect("tick2");
    assert!(s2.ok);
    assert_eq!(
        s2.rows_in, 1,
        "tick2 must pick up the previously-skipped log"
    );
    assert_eq!(
        count_agents(pool, CHAIN_ID as i64).await,
        2,
        "previously-unsafe log now committed"
    );
}

/// Sub-test: a dryrun does NOT touch the database or advance the cursor.
async fn case_dryrun(pool: &eth_tools_db::Pool, mock: &Arc<MockProvider>) {
    truncate_all(pool).await;
    mock.logs.lock().unwrap().clear();
    mock.set_head(BASE_BLOCK + 100);

    let addr = registry_addr();
    mock.push_log(make_log(
        Registered {
            agentId: U256::from(99u64),
            owner: Address::from([0x55u8; 20]),
            agentUri: "ipfs://dryrun".to_string(),
        },
        addr,
        BASE_BLOCK + 1,
        0,
        1_700_000_020,
    ));

    // Build a dryrun ctx by cloning the pool + rotator from existing deps.
    let deps = eth_tools_workers::context::deps().await.unwrap();
    let dryrun_ctx = WorkerContext {
        pool: deps.pool.clone(),
        rpc: deps.rpc.clone(),
        vercel_env: "production".into(),
        dryrun: true,
        force: false,
    };
    let summary = registry_scraper::run(&dryrun_ctx, CHAIN_ID)
        .await
        .expect("dryrun");
    assert!(summary.ok);
    assert!(summary.dryrun, "summary must reflect dryrun flag");
    // We DECODED the log (rows_in) but committed nothing.
    assert_eq!(summary.rows_in, 1, "dryrun still reads logs");
    assert_eq!(summary.rows_out, 0, "dryrun must commit zero rows");

    // Database must be empty AND cursors must NOT have advanced (so the next
    // real tick will replay the dryrun's range).
    assert_eq!(count_agents(pool, CHAIN_ID as i64).await, 0);
    let (cursor_rows,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM cursors")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(cursor_rows, 0, "dryrun must not advance any cursor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn registry_scraper_e2e() {
    let Some((_c, pool)) = boot().await else {
        return;
    };

    let mock = Arc::new(MockProvider::new(BASE_BLOCK + 100));
    let rotator = Arc::new(RotatingProvider::new(vec![mock.clone()]));
    let deps = WorkerDeps {
        pool: pool.clone(),
        rpc: rotator.clone(),
    };
    install_for_test(deps)
        .map_err(|_| "DEPS already initialized")
        .expect("install_for_test");

    let ctx = WorkerContext {
        pool: pool.clone(),
        rpc: rotator.clone(),
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    };

    case_all_four_events(&pool, &mock, &ctx).await;
    case_idempotency(&pool, &mock, &ctx).await;
    case_reorg_safety(&pool, &mock, &ctx).await;
    case_dryrun(&pool, &mock).await;
    case_metadata_set_non_wallet_key(&pool, &mock, &ctx).await;
}

/// Sub-test: `MetadataSet` with a key OTHER than `agentWallet` must record a
/// history row (change_kind='metadata') for audit but MUST NOT touch the
/// `agents` row's columns.
async fn case_metadata_set_non_wallet_key(
    pool: &eth_tools_db::Pool,
    mock: &Arc<MockProvider>,
    ctx: &WorkerContext,
) {
    truncate_all(pool).await;
    mock.logs.lock().unwrap().clear();
    mock.set_head(BASE_BLOCK + 100);

    let addr = registry_addr();
    let agent_id = U256::from(123u64);
    let owner = Address::from([0x66u8; 20]);

    // Register the agent so the MetadataSet has something to (not) touch.
    mock.push_log(make_log(
        Registered {
            agentId: agent_id,
            owner,
            agentUri: "ipfs://x".to_string(),
        },
        addr,
        BASE_BLOCK + 10,
        0,
        1_700_001_000,
    ));
    // Non-agentWallet metadata key.
    mock.push_log(make_log(
        MetadataSet {
            agentId: agent_id,
            key: "discord".to_string(),
            value: Bytes::from(b"satoshi#1234".to_vec()),
        },
        addr,
        BASE_BLOCK + 11,
        0,
        1_700_001_001,
    ));

    let summary = registry_scraper::run(ctx, CHAIN_ID).await.expect("run");
    assert!(summary.ok);
    // 2 logs decoded; 1 agents-table write (only the Registered insert).
    assert_eq!(summary.rows_in, 2);
    assert_eq!(summary.rows_out, 1);

    // History: 1 metadata row from Registered + 1 metadata row from the
    // non-wallet MetadataSet. Both have change_kind='metadata'.
    let kinds: Vec<String> = sqlx::query_scalar(
        "SELECT change_kind FROM agents_history WHERE chain_id = $1 ORDER BY id ASC",
    )
    .bind(CHAIN_ID as i64)
    .fetch_all(pool)
    .await
    .unwrap();
    assert_eq!(kinds, vec!["metadata", "metadata"]);

    // agent row reflects ONLY the Registered call.
    let (agent_uri, agent_wallet): (Option<String>, Option<Vec<u8>>) = sqlx::query_as(
        "SELECT agent_uri, agent_wallet FROM agents
          WHERE chain_id = $1 AND agent_id = $2",
    )
    .bind(CHAIN_ID as i64)
    .bind(BigDecimal::from(123))
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(agent_uri.as_deref(), Some("ipfs://x"));
    assert!(agent_wallet.is_none());
}
