//! Integration tests for W2 (manifest_fetcher).
//!
//! Boots a real Postgres via testcontainers + an in-process axum HTTP
//! server that simulates the agent's manifest endpoint. Drives the worker
//! end-to-end and asserts the resulting `manifests` + `agents` rows match
//! each scenario:
//!
//!   1. 200 OK with a valid ERC-8004 manifest          → status=valid
//!   2. 200 OK with a schema-violating manifest        → status=invalid
//!   3. 404                                            → status=unreachable
//!   4. Redirect to 127.0.0.1                          → status=unreachable
//!      (SSRF guard rejects the post-redirect hop)
//!   5. Content-Length=500 MB                          → status=unreachable
//!   6. Chunked body that streams past the 10 MB cap   → status=unreachable
//!
//! Plus a positive test that `agents.last_manifest_fetch` is bumped for
//! ALL outcomes (including unreachable) so the 24h staleness throttle
//! takes effect. Skipped automatically if Docker isn't available.
//!
//! ## Why a single sequential `#[tokio::test]`?
//!
//! Mirrors `serve_integration.rs`: the per-test container + the
//! `OnceCell<WorkerDeps>` in `context::DEPS` are process-global; parallel
//! tests would race. One test, six scenarios, each with its own agent row.

#![cfg(test)]

use bigdecimal::BigDecimal;
use chrono::Utc;
use eth_tools_core::safe_fetch::{
    fetch_with_guard, DnsResolver, FetchOptions, RealResolver, SafeFetchError, SsrfGuard,
};
use eth_tools_db::manifests::latest_for_agent;
use eth_tools_workers::manifest_fetcher::{self, Fetcher};
use eth_tools_workers::{WorkerContext, WorkerSummary};
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::sync::Arc;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;

/// Spin up Postgres + apply migrations. Returns `None` when Docker is
/// unavailable so CI without docker doesn't fail this test.
async fn boot_pg() -> Option<(impl std::fmt::Debug, eth_tools_db::Pool)> {
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

/// Insert a chain row + an agent row pointing at `uri`. Returns the
/// agent_id used so callers can correlate after the worker runs.
async fn insert_agent(pool: &eth_tools_db::Pool, chain_id: i64, agent_id: i64, uri: &str) -> BigDecimal {
    // Chain row is FK target; idempotent so multiple tests can share.
    let zero_addr: &[u8] = &[0u8; 20];
    sqlx::query(
        "INSERT INTO chains (chain_id, name, identity_registry, reputation_registry,
                             validation_registry, rpc_url, is_testnet)
         VALUES ($1, $2, $3, $3, $3, 'http://localhost/', false)
         ON CONFLICT (chain_id) DO NOTHING",
    )
    .bind(chain_id)
    .bind(format!("test-chain-{chain_id}"))
    .bind(zero_addr)
    .execute(pool)
    .await
    .expect("insert chain");

    let agent_id_bd: BigDecimal = agent_id.into();
    let owner: &[u8] = &[0xab; 20];
    sqlx::query(
        "INSERT INTO agents (chain_id, agent_id, owner, agent_uri, registered_at, updated_at)
         VALUES ($1, $2, $3, $4, $5, $5)
         ON CONFLICT (chain_id, agent_id) DO UPDATE SET agent_uri = EXCLUDED.agent_uri,
                                                        last_manifest_fetch = NULL",
    )
    .bind(chain_id)
    .bind(&agent_id_bd)
    .bind(owner)
    .bind(uri)
    .bind(Utc::now())
    .execute(pool)
    .await
    .expect("insert agent");
    agent_id_bd
}

async fn fetch_last_manifest_fetch(
    pool: &eth_tools_db::Pool,
    chain_id: i64,
    agent_id: &BigDecimal,
) -> Option<chrono::DateTime<Utc>> {
    let row: (Option<chrono::DateTime<Utc>>,) = sqlx::query_as(
        "SELECT last_manifest_fetch FROM agents WHERE chain_id = $1 AND agent_id = $2",
    )
    .bind(chain_id)
    .bind(agent_id)
    .fetch_one(pool)
    .await
    .expect("fetch last_manifest_fetch");
    row.0
}

/// Test-only fetcher: allows loopback (so axum fixtures on 127.0.0.1
/// resolve), still rejects RFC1918 so the scenario-4 redirect-to-10.0.0.1
/// trips the SSRF guard exactly like prod would. Same shared safe-fetch
/// machinery; the only knob is the guard policy.
fn test_fetcher() -> Fetcher {
    fn guard(ip: IpAddr) -> Result<(), String> {
        if let IpAddr::V4(v4) = ip {
            if v4.is_private() {
                return Err(format!("`{ip}` is RFC1918"));
            }
            // Loopback / link-local / anything else: allow (axum binds
            // to 127.0.0.1:<random> for fixtures).
        }
        Ok(())
    }
    Arc::new(|uri: String| {
        Box::pin(async move {
            let g: &SsrfGuard = &guard;
            let r: &dyn DnsResolver = &RealResolver;
            fetch_with_guard(&uri, r, g, &FetchOptions::default()).await
        }) as Pin<Box<dyn Future<Output = Result<Vec<u8>, SafeFetchError>> + Send>>
    })
}

fn ctx_for(pool: eth_tools_db::Pool) -> WorkerContext {
    use async_trait::async_trait;
    use eth_tools_rpc::{RotatingProvider, RpcError, RpcProvider};

    struct TestProvider;
    #[async_trait]
    impl RpcProvider for TestProvider {
        fn name(&self) -> &str {
            "test-provider"
        }
        async fn get_block_number(&self) -> Result<u64, RpcError> {
            Ok(0)
        }
        async fn get_logs(
            &self,
            _f: &alloy::rpc::types::Filter,
        ) -> Result<Vec<alloy::rpc::types::Log>, RpcError> {
            Ok(vec![])
        }
    }

    WorkerContext {
        pool,
        rpc: Arc::new(RotatingProvider::new(vec![Arc::new(TestProvider)])),
        vercel_env: "production".into(),
        dryrun: false,
        force: false,
    }
}

/// Spin up an HTTP server bound to 127.0.0.1:<random> that exposes every
/// fixture endpoint we need. Returns `(port, abort_handle)`.
async fn start_fixture_server() -> (u16, tokio::task::JoinHandle<()>) {
    use axum::{
        http::{header, StatusCode},
        response::Response,
        routing::get,
        Router,
    };

    let app = Router::new()
        // Scenario 1: 200 OK with a valid manifest body.
        .route(
            "/valid",
            get(|| async {
                let body = serde_json::to_vec(&serde_json::json!({
                    "name": "alice",
                    "description": "valid test agent",
                    "endpoints": [{"protocol": "https", "url": "https://x.example/agent"}]
                }))
                .unwrap();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }),
        )
        // Scenario 2: 200 OK but the body is missing `name` so schema
        // validation fires.
        .route(
            "/invalid_schema",
            get(|| async {
                let body = serde_json::to_vec(&serde_json::json!({
                    "endpoints": [{"protocol": "https", "url": "https://x.example/"}]
                }))
                .unwrap();
                Response::builder()
                    .status(StatusCode::OK)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(axum::body::Body::from(body))
                    .unwrap()
            }),
        )
        // Scenario 3: 404 → unreachable.
        .route(
            "/missing",
            get(|| async { (StatusCode::NOT_FOUND, "not found") }),
        )
        // Scenario 4: redirect to a private address; safe_fetch must
        // re-vet and reject the next hop.
        .route(
            "/redir_private",
            get(|| async {
                (
                    StatusCode::FOUND,
                    [(header::LOCATION, "http://10.0.0.1:1/secret")],
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (port, handle)
}

/// Raw-TCP server that announces a huge Content-Length and writes
/// nothing. The safe-fetch must short-circuit on the header alone. Reused
/// for scenario 5.
async fn start_huge_cl_server() -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 4096];
                let _ = sock.read(&mut tmp).await;
                let resp = "HTTP/1.1 200 OK\r\n\
                            Content-Type: application/json\r\n\
                            Content-Length: 536870912\r\n\
                            Connection: close\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, handle)
}

/// Raw-TCP server that omits Content-Length and streams forever via
/// chunked transfer-encoding. Reused for scenario 6.
async fn start_streamed_oversize_server() -> (u16, tokio::task::JoinHandle<()>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut tmp = [0u8; 4096];
                let _ = sock.read(&mut tmp).await;
                let headers = "HTTP/1.1 200 OK\r\n\
                               Content-Type: application/json\r\n\
                               Transfer-Encoding: chunked\r\n\
                               Connection: close\r\n\r\n";
                if sock.write_all(headers.as_bytes()).await.is_err() {
                    return;
                }
                // 64 KiB chunks. The cap is 10 MiB so ~160 chunks trips
                // it; we send extra in case of buffering before the
                // reader notices.
                let chunk = vec![b'x'; 64 * 1024];
                let chunk_hex = format!("{:x}\r\n", chunk.len());
                for _ in 0..300 {
                    if sock.write_all(chunk_hex.as_bytes()).await.is_err() {
                        return;
                    }
                    if sock.write_all(&chunk).await.is_err() {
                        return;
                    }
                    if sock.write_all(b"\r\n").await.is_err() {
                        return;
                    }
                }
                let _ = sock.write_all(b"0\r\n\r\n").await;
            });
        }
    });
    (port, handle)
}

#[tokio::test]
async fn manifest_fetcher_end_to_end_scenarios() {
    let Some((_pg, pool)) = boot_pg().await else {
        return;
    };

    // Ensure blob upload is OFF for the test (no Vercel token). The
    // worker reads this once at the top of `run()`.
    std::env::remove_var("VERCEL_BLOB_READ_WRITE_TOKEN");

    let (port, fixture_handle) = start_fixture_server().await;
    let (cl_port, cl_handle) = start_huge_cl_server().await;
    let (stream_port, stream_handle) = start_streamed_oversize_server().await;

    let base = format!("http://127.0.0.1:{port}");
    let chain_id = 8453_i64;

    // -----------------------------------------------------------------
    // Insert one agent per scenario. Unique agent_id so we can assert
    // per-scenario manifest rows independently.
    let id_valid = insert_agent(&pool, chain_id, 1, &format!("{base}/valid")).await;
    let id_invalid = insert_agent(&pool, chain_id, 2, &format!("{base}/invalid_schema")).await;
    let id_404 = insert_agent(&pool, chain_id, 3, &format!("{base}/missing")).await;
    let id_redir = insert_agent(&pool, chain_id, 4, &format!("{base}/redir_private")).await;
    let id_cl = insert_agent(&pool, chain_id, 5, &format!("http://127.0.0.1:{cl_port}/big")).await;
    let id_stream =
        insert_agent(&pool, chain_id, 6, &format!("http://127.0.0.1:{stream_port}/stream")).await;

    let before = Utc::now();

    let summary: WorkerSummary = manifest_fetcher::run_with(ctx_for(pool.clone()), test_fetcher())
        .await
        .expect("worker should not error");

    assert!(!summary.dryrun);
    assert!(summary.ok);
    // All 6 agents were stale (last_manifest_fetch was NULL on insert).
    assert_eq!(summary.rows_in, 6);
    // Every fetch should have produced a manifests row (valid+invalid+
    // unreachable all get persisted).
    assert_eq!(summary.rows_out, 6);

    // -----------------------------------------------------------------
    // Scenario 1: valid manifest.
    let row = latest_for_agent(&pool, chain_id, &id_valid)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "valid");
    assert!(row.validation_errors.is_none());
    // sha256 of the valid body must be non-empty-sentinel.
    assert_ne!(row.raw_bytes_sha256.len(), 0);
    assert_ne!(
        hex_str(&row.raw_bytes_sha256),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    // Scenario 2: schema-invalid.
    let row = latest_for_agent(&pool, chain_id, &id_invalid)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "invalid");
    let errs = row.validation_errors.expect("errors json");
    assert!(!errs.as_array().expect("array").is_empty());

    // Scenario 3: 404 → unreachable.
    let row = latest_for_agent(&pool, chain_id, &id_404)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "unreachable");
    // sentinel sha256 = sha256("")
    assert_eq!(
        hex_str(&row.raw_bytes_sha256),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );

    // Scenario 4: redirect-to-private → SSRF guard rejects the next hop.
    let row = latest_for_agent(&pool, chain_id, &id_redir)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "unreachable");
    let errs = row.validation_errors.expect("errors json");
    let first = errs.as_array().expect("array")[0]
        .as_str()
        .expect("string");
    assert!(
        first.contains("deny set")
            || first.contains("disallowed")
            || first.contains("10.0.0.1"),
        "expected SSRF-flavored error, got: {first}"
    );

    // Scenario 5: oversize Content-Length.
    let row = latest_for_agent(&pool, chain_id, &id_cl)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "unreachable");

    // Scenario 6: streamed oversize body.
    let row = latest_for_agent(&pool, chain_id, &id_stream)
        .await
        .expect("query")
        .expect("manifest row exists");
    assert_eq!(row.validation_status, "unreachable");

    // -----------------------------------------------------------------
    // Cross-cutting: every scenario must have bumped
    // last_manifest_fetch — even unreachable ones — so we don't
    // hot-loop on a broken URI.
    for id in [&id_valid, &id_invalid, &id_404, &id_redir, &id_cl, &id_stream] {
        let lmf = fetch_last_manifest_fetch(&pool, chain_id, id)
            .await
            .expect("last_manifest_fetch is set");
        assert!(
            lmf >= before,
            "last_manifest_fetch must advance past the test-start time"
        );
    }

    // -----------------------------------------------------------------
    // Re-run with the same data: the staleness query should now return
    // 0 agents (all were just visited), so rows_in=0 and rows_out=0.
    let summary2: WorkerSummary = manifest_fetcher::run_with(ctx_for(pool.clone()), test_fetcher())
        .await
        .expect("idempotent re-run");
    assert_eq!(summary2.rows_in, 0);
    assert_eq!(summary2.rows_out, 0);

    fixture_handle.abort();
    cl_handle.abort();
    stream_handle.abort();
}

fn hex_str(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s
}
