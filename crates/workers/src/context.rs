//! Per-invocation worker context + the lazily-built cold-start dependency
//! cache.
//!
//! This is the **M1 fix** from the Phase 4 audit: the previous
//! `cron::serve` closure signature only handed bodies a `bool` (dryrun).
//! Workers actually need a Postgres pool, a rotating RPC, the current head
//! block, and the `?force=1` flag. Forcing each of 8 worker bodies to roll
//! its own `OnceCell<PgPool>` would multiply the cold-start latency budget
//! and let init bugs hide per-worker; instead we centralise here.
//!
//! ## Why a shared `DEPS` cache (one pool + one rotator for all workers)?
//!
//! Vercel Fluid Compute keeps a function instance warm ≈ 14 days. Within
//! one instance, all 8 cron paths share the same process — so a single
//! `PgPool` of `max_connections=8` and a single `RotatingProvider` covers
//! every worker. Per-worker caches would each open their own pool, which
//! exhausts PgBouncer's 100-connection budget under burst load (8 workers
//! × 8 conns × N warm instances).
//!
//! ## Test seam: `install_for_test`
//!
//! `OnceCell` is process-global. Integration tests that boot a fresh
//! testcontainers Postgres per test would race the cache. `install_for_test`
//! lets a single integration-test binary inject one set of deps up-front;
//! the test harness in `tests/serve_integration.rs` is structured as one
//! sequential `#[tokio::test]` to match.

use std::sync::Arc;

use eth_tools_db::Pool as PgPool;
use eth_tools_rpc::RotatingProvider;
use tokio::sync::{OnceCell, SetError};
use vercel_runtime::Error;

use crate::rpc_http::StubProvider;

/// Per-request payload handed to every worker body. The body owns the
/// borrowed `pool` / `rpc` for the duration of the invocation and uses
/// `vercel_env` / `dryrun` / `force` for branching.
///
/// The lifetimes are erased via `'static` borrows from `WorkerDeps` —
/// fluid-compute warm instances live for hours, so the cost of cloning the
/// pool/Arc per request is negligible (Arc bump + pool clone is O(1)).
///
/// `Debug` is not derived because `RotatingProvider` carries internal
/// state (provider URLs) that we'd rather not surface in trace output;
/// the per-field manual impl below redacts those bits.
#[derive(Clone)]
pub struct WorkerContext {
    pub pool: PgPool,
    pub rpc: Arc<RotatingProvider>,
    pub vercel_env: String,
    pub dryrun: bool,
    pub force: bool,
}

impl std::fmt::Debug for WorkerContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerContext")
            .field("pool", &"PgPool { … }")
            .field("rpc", &"RotatingProvider { … }")
            .field("vercel_env", &self.vercel_env)
            .field("dryrun", &self.dryrun)
            .field("force", &self.force)
            .finish()
    }
}

/// Lazily-built singletons: opened once per cold start, shared across all 8
/// workers and across many invocations on the same warm instance.
#[derive(Clone)]
pub struct WorkerDeps {
    pub pool: PgPool,
    pub rpc: Arc<RotatingProvider>,
}

impl std::fmt::Debug for WorkerDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Don't dump the pool — its Debug pulls config strings that may
        // include credentials in the URL.
        f.debug_struct("WorkerDeps")
            .field("pool", &"PgPool { … }")
            .field("rpc", &"RotatingProvider { … }")
            .finish()
    }
}

static DEPS: OnceCell<WorkerDeps> = OnceCell::const_new();

/// Returns the process-global deps, building them on first call. Subsequent
/// callers (on the same warm instance) share the result.
///
/// Env vars read:
///   - `DATABASE_URL` — pgbouncer-friendly Postgres URL.
///   - `RPC_URL_PRIMARY` / `RPC_URL_FALLBACK` — currently logged only;
///     `StubProvider` carries no URL of its own (PR3 TODO).
pub async fn deps() -> Result<&'static WorkerDeps, Error> {
    DEPS.get_or_try_init(build_deps).await
}

async fn build_deps() -> Result<WorkerDeps, Error> {
    let database_url = std::env::var("DATABASE_URL").map_err(|_| Error::from("DATABASE_URL not set"))?;
    let pool = eth_tools_db::connect(&database_url)
        .await
        .map_err(|e| Error::from(format!("db connect failed: {e}")))?;

    // PR3 TODO: replace StubProvider with real alloy-HTTP impls built from
    // these URLs. We still read the env vars here so a misconfigured deploy
    // (missing RPC_URL_PRIMARY in production) trips an alert instead of
    // silently falling through.
    let primary_url = std::env::var("RPC_URL_PRIMARY").ok();
    let fallback_url = std::env::var("RPC_URL_FALLBACK").ok();
    if primary_url.is_none() {
        tracing::warn!("RPC_URL_PRIMARY not set; using StubProvider (PR3 will wire alloy)");
    }
    if fallback_url.is_none() {
        tracing::warn!("RPC_URL_FALLBACK not set");
    }

    let rpc = Arc::new(RotatingProvider::new(vec![Arc::new(StubProvider::new(
        "stub-primary",
    ))]));

    Ok(WorkerDeps { pool, rpc })
}

/// Test seam: inject pre-built deps before any `deps()` call. Returns
/// `Err(SetError)` if `DEPS` has already been initialised (a second test
/// can't replace the first test's container) — see `tokio::sync::SetError`.
///
/// Production code MUST NOT call this — it's gated by `#[doc(hidden)]` to
/// keep it out of rustdoc surface but visible to integration tests.
#[doc(hidden)]
pub fn install_for_test(deps: WorkerDeps) -> Result<(), SetError<WorkerDeps>> {
    DEPS.set(deps)
}
