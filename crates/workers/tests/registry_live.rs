//! Required DB integration: real HTTP RPC -> decoded events -> atomic PostgreSQL cursor.
use alloy::{rpc::types::Log, sol_types::SolEvent};
use alloy_primitives::{Address, B256, U256};
use eth_tools_core::{Registered, Transfer, URIUpdated};
use eth_tools_rpc::{HttpProvider, RotatingProvider};
use eth_tools_workers::{registry, WorkerContext};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    net::TcpListener,
    sync::Arc,
};

fn rpc_fixture(logs: Vec<Log>, head: u64) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let mut bytes = Vec::new();
            let request: Value = loop {
                let mut chunk = [0; 8192];
                let n = stream.read(&mut chunk).unwrap();
                assert_ne!(n, 0);
                bytes.extend_from_slice(&chunk[..n]);
                if let Some(end) = bytes.windows(4).position(|v| v == b"\r\n\r\n") {
                    let header = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                    let size: usize = header
                        .lines()
                        .find_map(|s| s.strip_prefix("content-length:"))
                        .unwrap()
                        .trim()
                        .parse()
                        .unwrap();
                    if bytes.len() >= end + 4 + size {
                        break serde_json::from_slice(&bytes[end + 4..end + 4 + size]).unwrap();
                    }
                }
            };
            let result = match request["method"].as_str().unwrap() {
                "eth_blockNumber" => json!(format!("0x{head:x}")),
                "eth_getLogs" => {
                    let f = &request["params"][0];
                    let start =
                        u64::from_str_radix(f["fromBlock"].as_str().unwrap().trim_start_matches("0x"), 16)
                            .unwrap();
                    let end =
                        u64::from_str_radix(f["toBlock"].as_str().unwrap().trim_start_matches("0x"), 16)
                            .unwrap();
                    json!(logs
                        .iter()
                        .filter(|l| l.block_number.is_some_and(|b| b >= start && b <= end))
                        .collect::<Vec<_>>())
                }
                other => panic!("unexpected RPC method {other}"),
            };
            let body = json!({"jsonrpc":"2.0","id":request["id"],"result":result}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        }
    });
    url
}
fn log<E: SolEvent>(event: E, index: u64, block: u64) -> Log {
    Log {
        inner: alloy_primitives::Log {
            address: Address::from(eth_tools_core::CHAINS[0].identity_registry),
            data: event.encode_log_data(),
        },
        block_hash: Some(B256::repeat_byte(2)),
        block_number: Some(block),
        block_timestamp: Some(1700000000),
        transaction_hash: Some(B256::repeat_byte(3)),
        transaction_index: Some(0),
        log_index: Some(index),
        removed: false,
    }
}
#[tokio::test]
#[ignore = "requires isolated PostgreSQL; dedicated native E2E CI runs --ignored"]
async fn required_live_postgres_identity_batch_replay_and_dryrun() {
    // Missing DATABASE_URL fails; this test must never silently skip.
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL required for real registry integration");
    let pool = eth_tools_db::connect(&url).await.unwrap();
    eth_tools_db::migrate(&pool).await.unwrap();
    let chain = eth_tools_core::CHAINS[0];
    let block = chain.genesis_block;
    let id = U256::from(424242u64);
    let owner = Address::repeat_byte(4);
    let buyer = Address::repeat_byte(5);
    sqlx::query("DELETE FROM cursors WHERE cursor_key=$1")
        .bind(format!("identity:{}", chain.chain_id))
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM agents WHERE chain_id=$1 AND agent_id=424242")
        .bind(chain.chain_id as i64)
        .execute(&pool)
        .await
        .unwrap();
    let logs = vec![
        log(
            Registered {
                agentId: id,
                owner,
                agentUri: "https://example.invalid/first.json".into(),
            },
            0,
            block,
        ),
        log(
            URIUpdated {
                agentId: id,
                updatedBy: owner,
                newUri: "https://example.invalid/final.json".into(),
            },
            1,
            block,
        ),
        log(
            Transfer {
                from: owner,
                to: buyer,
                tokenId: id,
            },
            2,
            block,
        ),
    ];
    let endpoint = rpc_fixture(logs, block + 12);
    let context = WorkerContext {
        pool: pool.clone(),
        rpc: Arc::new(RotatingProvider::new(vec![Arc::new(
            HttpProvider::new("local-fixture", &endpoint).unwrap(),
        )])),
        vercel_env: "local".into(),
        dryrun: true,
        force: false,
    };
    assert_eq!(registry::run(context.clone()).await.unwrap().rows_out, 3);
    assert!(eth_tools_db::agents::get_one(
        &pool,
        chain.chain_id as i64,
        &bigdecimal::BigDecimal::from(424242)
    )
    .await
    .unwrap()
    .is_none());
    let mut real = context.clone();
    real.dryrun = false;
    assert_eq!(registry::run(real.clone()).await.unwrap().rows_out, 3);
    let row = eth_tools_db::agents::get_one(
        &pool,
        chain.chain_id as i64,
        &bigdecimal::BigDecimal::from(424242),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(row.owner, buyer.as_slice());
    assert_eq!(
        row.agent_uri.as_deref(),
        Some("https://example.invalid/final.json")
    );
    assert!(row.agent_wallet.is_none());
    // New invocation resumes committed cursor rather than replaying writes.
    assert_eq!(registry::run(real).await.unwrap().rows_out, 0);
    // A later malformed batch must roll back its preceding valid insert AND cursor.
    let next = block + 1;
    let bad_logs = vec![
        log(
            Registered {
                agentId: U256::from(424243u64),
                owner,
                agentUri: "https://example.invalid/rollback.json".into(),
            },
            0,
            next,
        ),
        log(
            URIUpdated {
                agentId: U256::from(424244u64),
                updatedBy: owner,
                newUri: "https://example.invalid/unknown.json".into(),
            },
            1,
            next,
        ),
    ];
    let bad_endpoint = rpc_fixture(bad_logs, next + 12);
    let bad_context = WorkerContext {
        pool: pool.clone(),
        rpc: Arc::new(RotatingProvider::new(vec![Arc::new(
            HttpProvider::new("bad-batch", &bad_endpoint).unwrap(),
        )])),
        vercel_env: "local".into(),
        dryrun: false,
        force: false,
    };
    assert!(registry::run(bad_context).await.is_err());
    assert!(eth_tools_db::agents::get_one(
        &pool,
        chain.chain_id as i64,
        &bigdecimal::BigDecimal::from(424243)
    )
    .await
    .unwrap()
    .is_none());
    let mut connection = pool.acquire().await.unwrap();
    let cursor = eth_tools_db::cursors::get(&mut connection, &format!("identity:{}", chain.chain_id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(cursor.last_block, block as i64);
    drop(connection);
    pool.close().await;
    let reopened = eth_tools_db::connect(&url).await.unwrap();
    assert!(eth_tools_db::agents::get_one(
        &reopened,
        chain.chain_id as i64,
        &bigdecimal::BigDecimal::from(424242)
    )
    .await
    .unwrap()
    .is_some());
}
