//! End-to-end test for the worker substrate:
//!   - `cron::serve` builds a `WorkerContext` and hands it to the body.
//!   - Non-dryrun invocations insert exactly one `worker_runs` row with
//!     `ok=true, finished_at IS NOT NULL`.
//!   - Dry-run invocations return 200 but do NOT write to `worker_runs`
//!     (plan §3 invariant 4: "runs all reads but skips writes").
//!   - `cursors::{advance,get}` round-trip an `(chain, contract, topic0)`
//!     triple through `cursor_key()`.
//!
//! Skipped automatically if Docker isn't available (testcontainers returns
//! an error which we treat as "skipped"). Locally requires `docker` running.
//!
//! ## Why a single `#[tokio::test]` instead of one-per-assertion?
//!
//! `cron::serve` and `context::deps()` both touch process-global state
//! (env vars + a `OnceCell<WorkerDeps>`). Splitting into parallel tests
//! within one binary would race: test A's `OnceCell::set` blocks test B's,
//! and `std::env::set_var("VERCEL_ENV", ...)` in one test flips the value
//! mid-flight for another. One sequential test covers all cases — mirrors
//! the `cron_guard_enforces_secret_and_env` pattern in `crates/workers/
//! src/cron.rs`.

#![cfg(test)]

use std::sync::Arc;

use eth_tools_db::{cursors, worker_runs as wr};
use eth_tools_workers::context::{install_for_test, WorkerDeps};
use eth_tools_workers::cron::serve_with_context;
use eth_tools_workers::WorkerSummary;
use testcontainers::runners::AsyncRunner;
use testcontainers::ImageExt;
use testcontainers_modules::postgres::Postgres;
use vercel_runtime::{Body, Request, StatusCode};

const WORKER_NAME: &str = "test_w";

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

fn req_with_secret(secret: &'static str, query: Option<&str>) -> Request {
    let uri = match query {
        Some(q) => format!("https://example.com/api/cron/test?{q}"),
        None => "https://example.com/api/cron/test".into(),
    };
    http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("x-vercel-cron-secret", secret)
        .body(Body::Empty)
        .unwrap()
}

async fn count_runs(pool: &eth_tools_db::Pool) -> i64 {
    let (c,): (i64,) = sqlx::query_as("SELECT COUNT(*)::BIGINT FROM worker_runs WHERE worker_name = $1")
        .bind(WORKER_NAME)
        .fetch_one(pool)
        .await
        .expect("count");
    c
}

#[tokio::test]
async fn serve_writes_worker_runs_and_skips_on_dryrun() {
    let Some((_c, pool)) = boot().await else {
        return;
    };

    // Install deps before any `cron::serve` call — `context::deps()` would
    // otherwise try to read `DATABASE_URL`. The Stub RotatingProvider is
    // fine here; this test never touches RPC.
    use async_trait::async_trait;
    use eth_tools_rpc::RotatingProvider;
    use eth_tools_rpc::{RpcError, RpcProvider};

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

    let deps = WorkerDeps {
        pool: pool.clone(),
        rpc: Arc::new(RotatingProvider::new(vec![Arc::new(TestProvider)])),
    };
    install_for_test(deps)
        .map_err(|_| "install_for_test: DEPS already initialized")
        .expect("install_for_test");

    // Production + correct secret → body runs, audit row written.
    std::env::set_var("VERCEL_ENV", "production");
    std::env::set_var("CRON_SECRET", "right");

    let before = count_runs(&pool).await;

    let resp = serve_with_context(WORKER_NAME, req_with_secret("right", None), |ctx| {
        let pool = ctx.pool.clone();
        async move {
            // Body sees the real pool: confirm by issuing a trivial query.
            let (one,): (i32,) = sqlx::query_as("SELECT 1")
                .fetch_one(&pool)
                .await
                .expect("body select 1");
            assert_eq!(one, 1);
            assert!(!ctx.dryrun);
            assert!(!ctx.force);
            assert_eq!(ctx.vercel_env, "production");
            Ok(WorkerSummary {
                worker: WORKER_NAME,
                ok: true,
                rows_in: 7,
                rows_out: 3,
                dryrun: false,
                skipped: false,
                reason: None,
            })
        }
    })
    .await;
    assert_eq!(resp.status(), StatusCode::OK);

    let after = count_runs(&pool).await;
    assert_eq!(
        after,
        before + 1,
        "non-dryrun must write exactly one worker_runs row"
    );

    // Verify the row is terminal (ok=true, finished_at set, row counts
    // preserved). Pull the latest by id.
    type LatestRunRow = (
        Option<bool>,
        Option<chrono::DateTime<chrono::Utc>>,
        Option<i32>,
        Option<i32>,
        bool,
    );
    let latest: LatestRunRow = sqlx::query_as(
        "SELECT ok, finished_at, rows_in, rows_out, dryrun
           FROM worker_runs
          WHERE worker_name = $1
          ORDER BY id DESC LIMIT 1",
    )
    .bind(WORKER_NAME)
    .fetch_one(&pool)
    .await
    .expect("fetch latest run");
    let (ok, finished_at, rows_in, rows_out, dryrun) = latest;
    assert_eq!(ok, Some(true));
    assert!(finished_at.is_some());
    assert_eq!(rows_in, Some(7));
    assert_eq!(rows_out, Some(3));
    assert!(!dryrun);

    // ---------------- Dry-run: no audit row ------------------------------
    let before = count_runs(&pool).await;
    let resp = serve_with_context(
        WORKER_NAME,
        req_with_secret("right", Some("dryrun=1")),
        |ctx| async move {
            assert!(ctx.dryrun, "body must see dryrun=true");
            Ok(WorkerSummary::ok(WORKER_NAME))
        },
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let after = count_runs(&pool).await;
    assert_eq!(after, before, "dryrun must NOT write a worker_runs row");

    // ---------------- finish_err writes ok=false -------------------------
    let before = count_runs(&pool).await;
    let resp = serve_with_context(WORKER_NAME, req_with_secret("right", None), |_ctx| async {
        Err(vercel_runtime::Error::from("synthetic failure"))
    })
    .await;
    // Body returned Err → serve_inner propagates → outer serve maps to 500.
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let after = count_runs(&pool).await;
    assert_eq!(after, before + 1, "errored body still writes one audit row");
    let (ok, error): (Option<bool>, Option<String>) = sqlx::query_as(
        "SELECT ok, error FROM worker_runs WHERE worker_name = $1
          ORDER BY id DESC LIMIT 1",
    )
    .bind(WORKER_NAME)
    .fetch_one(&pool)
    .await
    .expect("fetch errored run");
    assert_eq!(ok, Some(false));
    assert!(error.unwrap_or_default().contains("synthetic failure"));

    // ---------------- cursors round-trip ---------------------------------
    let contract = [0x11u8; 20];
    let topic = [0x22u8; 32];
    let key = cursors::cursor_key(8453, &contract, &topic);

    let mut conn = pool.acquire().await.expect("acquire");
    assert!(cursors::get(&mut conn, &key).await.expect("get").is_none());
    cursors::advance(&mut conn, &key, 1_234_567, 4)
        .await
        .expect("advance");
    let got = cursors::get(&mut conn, &key)
        .await
        .expect("get after advance")
        .expect("cursor row exists");
    assert_eq!(got.cursor_key, key);
    assert_eq!(got.last_block, 1_234_567);
    assert_eq!(got.last_log_index, 4);
    // Re-advance updates in place.
    cursors::advance(&mut conn, &key, 1_234_900, 0)
        .await
        .expect("re-advance");
    let got2 = cursors::get(&mut conn, &key).await.expect("get2").unwrap();
    assert_eq!(got2.last_block, 1_234_900);
    assert_eq!(got2.last_log_index, 0);
    assert!(got2.updated_at >= got.updated_at);

    // ---------------- worker_runs helpers direct -------------------------
    let h = wr::begin(&pool, "direct_helper", "production", false)
        .await
        .expect("begin");
    let run_id = h.run_id();
    assert!(run_id > 0);
    wr::finish_ok(&pool, h, 11, 22).await.expect("finish_ok");
    let (rows_in, rows_out, ok): (Option<i32>, Option<i32>, Option<bool>) =
        sqlx::query_as("SELECT rows_in, rows_out, ok FROM worker_runs WHERE id = $1")
            .bind(run_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(rows_in, Some(11));
    assert_eq!(rows_out, Some(22));
    assert_eq!(ok, Some(true));

    let h2 = wr::begin(&pool, "direct_helper", "production", true)
        .await
        .expect("begin dryrun");
    let big = "X".repeat(64 * 1024);
    wr::finish_err(&pool, h2, &big).await.expect("finish_err");

    std::env::remove_var("VERCEL_ENV");
    std::env::remove_var("CRON_SECRET");
}
