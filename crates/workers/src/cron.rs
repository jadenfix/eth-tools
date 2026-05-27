//! Shared cron entrypoint helper. Every `api/cron/*.rs` calls one of:
//!   - [`serve`] — the **legacy** PR1 signature, closure takes a `bool`
//!     (dryrun). Retained for compatibility with the 8 stub wrappers in
//!     `api/cron/*.rs` that PR3 will migrate.
//!   - [`serve_with_context`] — **the M1 fix from the Phase 4 audit.** The
//!     closure receives a [`WorkerContext`] (pool, rpc, vercel_env,
//!     dryrun, force). All new worker bodies use this entrypoint; PR3
//!     migrates the 8 stubs over and renames `serve_with_context` →
//!     `serve` (deleting the legacy adapter).
//!
//! Both entrypoints share `serve_inner` so the four guards from plan §3
//! evaluate exactly once:
//!  1. Cron secret check (**fail-closed in production**).
//!  2. Env-aware skip (non-prod returns `{skipped: true}`) — bypassed by
//!     `?force=1` (plan §3.2: "Backfill / replay is opt-in").
//!  3. Dry-run via `?dryrun=1`.
//!  4. Uniform telemetry envelope — writes one `worker_runs` row per
//!     non-dryrun invocation; dryruns return 200 with no audit row.

use std::future::Future;

use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use vercel_runtime::{Body, Error, Request, Response, StatusCode};

use crate::context::{self, WorkerContext};
use crate::WorkerSummary;
use eth_tools_db::worker_runs;

/// **Legacy PR1 entrypoint.** Closure takes a `bool` (`dryrun`); used by
/// the 8 stub wrappers in `api/cron/*.rs`. New worker bodies should call
/// [`serve_with_context`] instead — it provides the pool + rpc + force flag.
///
/// PR3 deletes this shim once the wrappers migrate.
pub async fn serve<F, Fut>(worker_name: &'static str, req: Request, body: F) -> Result<Response<Body>, Error>
where
    F: FnOnce(bool) -> Fut,
    Fut: Future<Output = Result<WorkerSummary, Error>>,
{
    // Adapt the bool-closure API onto the new WorkerContext substrate.
    // We discard the pool/rpc — legacy bodies never used them, and the
    // OnceCell init in serve_with_context() is safe to skip via the early
    // "skipped" path we want to preserve for non-prod stubs.
    //
    // BUT we must NOT call serve_with_context here, because that would
    // try to init `context::deps()` even for the stub callers (which have
    // no DATABASE_URL in dev). Instead we run the guards inline.
    Ok(serve_legacy_inner(worker_name, req, body)
        .await
        .unwrap_or_else(|e| internal_error(worker_name, &e.to_string())))
}

/// **New M1-refactor entrypoint.** Closure receives a [`WorkerContext`]
/// (pool, rpc, vercel_env, dryrun, force) and returns the summary it would
/// have written.
///
/// On any internal failure (auth, deps init, body error) returns a
/// well-formed `Response` rather than propagating `Result::Err` — Vercel
/// cron paths must always emit a status, never a panic.
pub async fn serve_with_context<F, Fut>(worker_name: &'static str, req: Request, body: F) -> Response<Body>
where
    F: FnOnce(WorkerContext) -> Fut,
    Fut: Future<Output = Result<WorkerSummary, Error>>,
{
    match serve_inner(worker_name, req, body).await {
        Ok(resp) => resp,
        Err(e) => internal_error(worker_name, &e.to_string()),
    }
}

/// Legacy code path: runs the four guards, but does NOT touch
/// `context::deps()` (legacy bodies don't need a pool/rpc, and stub
/// wrappers in dev have no DATABASE_URL). Dryrun + audit are still skipped
/// identically to the new path.
async fn serve_legacy_inner<F, Fut>(
    worker_name: &'static str,
    req: Request,
    body: F,
) -> Result<Response<Body>, Error>
where
    F: FnOnce(bool) -> Fut,
    Fut: Future<Output = Result<WorkerSummary, Error>>,
{
    let env = std::env::var("VERCEL_ENV").unwrap_or_else(|_| "development".into());
    let is_prod = env == "production";

    if let Some(resp) = check_cron_secret(worker_name, &req, &env, is_prod)? {
        return Ok(resp);
    }
    let (dryrun, force) = parse_query_flags(req.uri().query());
    if !is_prod && !force {
        let summary = WorkerSummary::skipped(worker_name, "non-production");
        return ok_json(&summary);
    }
    // Legacy path: no deps, no worker_runs audit row. PR3 deletes this fn
    // when the 8 stubs migrate to serve_with_context.
    let mut summary = body(dryrun).await?;
    if dryrun {
        summary.dryrun = true;
    }
    ok_json(&summary)
}

async fn serve_inner<F, Fut>(
    worker_name: &'static str,
    req: Request,
    body: F,
) -> Result<Response<Body>, Error>
where
    F: FnOnce(WorkerContext) -> Fut,
    Fut: Future<Output = Result<WorkerSummary, Error>>,
{
    let env = std::env::var("VERCEL_ENV").unwrap_or_else(|_| "development".into());
    let is_prod = env == "production";

    if let Some(resp) = check_cron_secret(worker_name, &req, &env, is_prod)? {
        return Ok(resp);
    }
    let (dryrun, force) = parse_query_flags(req.uri().query());

    // Env-aware skip. Plan §3.2: `?force=1` opts into running in non-prod.
    // Cron secret is still required (already checked above).
    if !is_prod && !force {
        let summary = WorkerSummary::skipped(worker_name, "non-production");
        return ok_json(&summary);
    }

    // Build context.
    let deps = context::deps().await?;
    let ctx = WorkerContext {
        pool: deps.pool.clone(),
        rpc: deps.rpc.clone(),
        vercel_env: env.clone(),
        dryrun,
        force,
    };

    // Dry-run skips the audit row (plan §3 invariant 4: "runs all reads
    // but skips writes"). HTTP response is still 200 so callers can verify
    // the read path. Dryrun also skips the advisory lock — read-only paths
    // are inherently idempotent and double-execution is harmless.
    if dryrun {
        let mut summary = body(ctx).await?;
        summary.dryrun = true;
        return ok_json(&summary);
    }

    // Plan §3 invariant #1: concurrency lock via Postgres advisory lock.
    // Vercel triggers a second cron instance while the first is still
    // running (per https://vercel.com/docs/cron-jobs#cron-job-considerations);
    // without this lock two scrapers would race and double-write events.
    //
    // We use `pg_try_advisory_xact_lock` (transaction-scoped) rather than
    // session-scoped because Neon's pooled DATABASE_URL runs PgBouncer in
    // transaction mode — session locks would leak across connections.
    // The lock holder transaction is kept open for the entire worker body;
    // it commits at the end, releasing the lock atomically. If the function
    // panics or aborts, the transaction rolls back (also releasing).
    //
    // `hashtext` returns int4; cast to bigint for the single-arg
    // `pg_try_advisory_xact_lock(int8)` form. Collision space is 2^32 — at
    // 8 workers it is effectively zero.
    let mut lock_tx = deps
        .pool
        .begin()
        .await
        .map_err(|e| Error::from(format!("lock tx begin failed: {e}")))?;
    let lock_key = format!("worker_{worker_name}");
    let (acquired,): (bool,) = sqlx::query_as("SELECT pg_try_advisory_xact_lock(hashtext($1)::bigint)")
        .bind(&lock_key)
        .fetch_one(&mut *lock_tx)
        .await
        .map_err(|e| Error::from(format!("advisory lock query failed: {e}")))?;
    if !acquired {
        // Another invocation holds the lock. Return 200 with skipped so
        // Vercel doesn't retry (it doesn't anyway, but be explicit).
        // Do not write a worker_runs row — the holder is the canonical run.
        let _ = lock_tx.rollback().await;
        tracing::info!(worker = worker_name, "skipped: advisory lock held");
        let summary = WorkerSummary::skipped(worker_name, "already_running");
        return ok_json(&summary);
    }

    // Telemetry envelope: open the audit row, run body, close.
    let handle = match worker_runs::begin(&deps.pool, worker_name, &env, false).await {
        Ok(h) => h,
        Err(e) => {
            // If we can't even open the audit row we still try to run the
            // body — but log loudly so the watchdog can detect missing
            // telemetry. Returning 500 here would mask actual worker
            // progress.
            tracing::error!(worker = worker_name, error = %e, "worker_runs::begin failed; running un-audited");
            let summary = body(ctx).await?;
            // Best-effort release; if commit fails the rollback on drop
            // still releases the xact lock.
            if let Err(ce) = lock_tx.commit().await {
                tracing::warn!(worker = worker_name, error = %ce, "lock tx commit failed; rollback will still release");
            }
            return ok_json(&summary);
        }
    };

    let result = body(ctx).await;
    match &result {
        Ok(summary) => {
            // Saturating cast to i32 — a worker that processed >2B rows in
            // one tick is a bug elsewhere; saturating keeps the audit row.
            let rows_in = summary.rows_in.min(i32::MAX as u64) as i32;
            let rows_out = summary.rows_out.min(i32::MAX as u64) as i32;
            if let Err(e) = worker_runs::finish_ok(&deps.pool, handle, rows_in, rows_out).await {
                tracing::error!(worker = worker_name, error = %e, "worker_runs::finish_ok failed");
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if let Err(audit_err) = worker_runs::finish_err(&deps.pool, handle, &msg).await {
                tracing::error!(
                    worker = worker_name,
                    error = %audit_err,
                    original_error = %msg,
                    "worker_runs::finish_err failed"
                );
            }
        }
    }
    // Release the advisory lock. If commit fails we log and let the tx
    // Drop rollback; either way the lock is released before we return.
    if let Err(ce) = lock_tx.commit().await {
        tracing::warn!(worker = worker_name, error = %ce, "lock tx commit failed; rollback will release");
    }
    let summary = result?;
    ok_json(&summary)
}

/// Returns `Ok(Some(error_response))` if auth failed, `Ok(None)` to
/// continue. The single shared guard for both `serve` paths.
fn check_cron_secret(
    worker_name: &'static str,
    req: &Request,
    env: &str,
    is_prod: bool,
) -> Result<Option<Response<Body>>, Error> {
    // Vercel cron auto-injects `Authorization: Bearer ${CRON_SECRET}` per
    // https://vercel.com/docs/cron-jobs/manage-cron-jobs#securing-cron-jobs.
    // Earlier drafts read a custom `x-vercel-cron-secret` header — Vercel
    // does NOT send this; reads of that header in production silently 401
    // every real cron tick. PR #1 fixed it on the infrastructure branch;
    // this fn was reintroduced in the worker-substrate rewrite and
    // regressed the fix until the Phase-4-PR2 audit caught it.
    let provided = req
        .headers()
        .get(http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer ").or_else(|| s.strip_prefix("bearer ")));
    let expected = std::env::var("CRON_SECRET").ok();
    let secret_ok = match (expected.as_deref(), provided) {
        (Some(exp), Some(got)) => constant_time_eq_str(exp, got),
        (None, _) if !is_prod => true,
        _ => false,
    };
    if !is_prod && expected.is_none() {
        tracing::warn!(
            worker = worker_name,
            "CRON_SECRET unset; allowed in non-prod only"
        );
    }
    if secret_ok {
        return Ok(None);
    }
    let code = if is_prod && expected.is_none() {
        "CRON_SECRET_MISCONFIGURED"
    } else {
        "INVALID_CRON_SECRET"
    };
    tracing::warn!(worker = worker_name, vercel_env = %env, code, "cron auth failed");
    let body_json = serde_json::json!({ "error": code });
    let status = if code == "CRON_SECRET_MISCONFIGURED" {
        StatusCode::INTERNAL_SERVER_ERROR
    } else {
        StatusCode::UNAUTHORIZED
    };
    Ok(Some(
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::Text(body_json.to_string()))?,
    ))
}

fn ok_json<T: serde::Serialize>(value: &T) -> Result<Response<Body>, Error> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(serde_json::to_string(value)?))?)
}

fn internal_error(worker_name: &str, msg: &str) -> Response<Body> {
    tracing::error!(worker = worker_name, error = msg, "worker serve failed");
    let body = serde_json::json!({ "error": "WORKER_INTERNAL_ERROR", "worker": worker_name });
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header("content-type", "application/json")
        .body(Body::Text(body.to_string()))
        .expect("static response is always valid")
}

/// Parse `?dryrun=1|true` and `?force=1|true` from the raw query string.
/// Other params are ignored (forward-compatible with PR3 additions).
fn parse_query_flags(query: Option<&str>) -> (bool, bool) {
    let Some(q) = query else {
        return (false, false);
    };
    let mut dryrun = false;
    let mut force = false;
    for kv in q.split('&') {
        match kv {
            "dryrun=1" | "dryrun=true" => dryrun = true,
            "force=1" | "force=true" => force = true,
            _ => {}
        }
    }
    (dryrun, force)
}

/// Length-hiding constant-time string compare.
///
/// `subtle::ConstantTimeEq` on byte slices short-circuits on length mismatch,
/// which leaks the secret's length via timing. Hashing both sides to a fixed
/// 32-byte digest first removes that side-channel: the comparison is always
/// over 32 bytes regardless of input length. SHA-256 itself processes in
/// data-independent time (block-padded).
fn constant_time_eq_str(a: &str, b: &str) -> bool {
    let ha = Sha256::digest(a.as_bytes());
    let hb = Sha256::digest(b.as_bytes());
    ha.ct_eq(&hb).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req_with(headers: &[(&'static str, &'static str)], query: Option<&str>) -> Request {
        let mut builder = http::Request::builder().method("POST").uri(match query {
            Some(q) => format!("https://example.com/api/cron/test?{q}"),
            None => "https://example.com/api/cron/test".into(),
        });
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        builder.body(Body::Empty).unwrap()
    }

    // Single test: env-var mutation is process-global and would race if
    // split across parallel #[test] cases. Mirror of the original PR1 test
    // matrix, with all 8 cases preserved against the legacy `serve` API
    // so the 8 stub wrappers in `api/cron/*.rs` keep working unchanged.
    #[tokio::test]
    async fn cron_guard_enforces_secret_and_env() {
        // Case 1: production with unset CRON_SECRET → 500 (fail-closed).
        std::env::set_var("VERCEL_ENV", "production");
        std::env::remove_var("CRON_SECRET");
        let r = serve("test_w", req_with(&[], None), |_| async {
            Ok(WorkerSummary::ok("test_w"))
        })
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Case 2: production, wrong secret → 401.
        std::env::set_var("CRON_SECRET", "right");
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer wrong")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 3: production, correct secret → 200 (body ran, legacy path
        // does not write audit row).
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer right")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 4: production, correct secret, ?dryrun=1 → body sees
        // dryrun=true.
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer right")], Some("dryrun=1")),
            |dr| async move {
                assert!(dr, "body should see dryrun=true");
                Ok(WorkerSummary::ok("test_w"))
            },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 5: preview without secret → 200 skipped (body NOT run).
        std::env::set_var("VERCEL_ENV", "preview");
        std::env::remove_var("CRON_SECRET");
        let r = serve("test_w", req_with(&[], None), |_| async {
            panic!("body should not run when env=preview");
        })
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 6: production + CRON_SECRET set + header missing → 401
        // (regression coverage for the `(Some, None)` match arm).
        std::env::set_var("VERCEL_ENV", "production");
        std::env::set_var("CRON_SECRET", "configured");
        let r = serve("test_w", req_with(&[], None), |_| async {
            panic!("body should not run without header");
        })
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 7: secret of different length must be rejected (length-
        // hiding ct compare must still return false — correctness, not
        // timing).
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer configuredXX")], None),
            |_| async { panic!("body should not run on length-mismatched secret") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 8: HTTP headers are case-insensitive — capitalized header
        // name and lowercase Bearer scheme both work.
        let r = serve(
            "test_w",
            req_with(&[("Authorization", "Bearer configured")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        let r = serve(
            "test_w",
            req_with(&[("authorization", "bearer configured")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 8b: regression — legacy custom `x-vercel-cron-secret`
        // header must NOT auth. Vercel does not send it; the worker
        // substrate's serve_inner rewrite reintroduced reads of this
        // header until the Phase-4-PR2 audit caught it.
        let r = serve(
            "test_w",
            req_with(&[("x-vercel-cron-secret", "configured")], None),
            |_| async { panic!("legacy x-vercel-cron-secret must NOT auth") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 8c: malformed Authorization header (no Bearer scheme) → 401.
        let r = serve(
            "test_w",
            req_with(&[("authorization", "configured")], None),
            |_| async { panic!("Authorization without Bearer scheme must NOT auth") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 9: preview + ?force=1, no secret → bypasses env-skip; body
        // runs (legacy path, no deps).
        std::env::set_var("VERCEL_ENV", "preview");
        std::env::remove_var("CRON_SECRET");
        let r = serve("test_w", req_with(&[], Some("force=1")), |dr| async move {
            assert!(!dr);
            Ok(WorkerSummary::ok("test_w"))
        })
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 10: serve_with_context + preview + ?force=1, no secret,
        // no DATABASE_URL → bypasses env-skip, attempts deps() which
        // errors, surfaces a 500. (Distinguishes force-bypass from the
        // env-skip's 200/skipped.)
        std::env::set_var("VERCEL_ENV", "preview");
        std::env::remove_var("CRON_SECRET");
        std::env::remove_var("DATABASE_URL");
        let r = serve_with_context("test_w", req_with(&[], Some("force=1")), |_ctx| async {
            Ok(WorkerSummary::ok("test_w"))
        })
        .await;
        assert_eq!(r.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Case 11: serve_with_context + preview, no ?force → 200 skipped
        // (body never reached, never touches deps()).
        let r = serve_with_context("test_w", req_with(&[], None), |_ctx| async {
            panic!("body must not run on env-skip");
        })
        .await;
        assert_eq!(r.status(), StatusCode::OK);

        std::env::remove_var("VERCEL_ENV");
        std::env::remove_var("CRON_SECRET");
    }

    #[test]
    fn parse_query_flags_handles_both() {
        assert_eq!(parse_query_flags(None), (false, false));
        assert_eq!(parse_query_flags(Some("")), (false, false));
        assert_eq!(parse_query_flags(Some("dryrun=1")), (true, false));
        assert_eq!(parse_query_flags(Some("force=1")), (false, true));
        assert_eq!(parse_query_flags(Some("dryrun=true&force=true")), (true, true));
        assert_eq!(parse_query_flags(Some("force=1&dryrun=1")), (true, true));
        // Unknown flags ignored.
        assert_eq!(parse_query_flags(Some("foo=bar&dryrun=1")), (true, false));
        // Misspellings don't accidentally trip.
        assert_eq!(parse_query_flags(Some("forced=1")), (false, false));
    }

    #[test]
    fn constant_time_eq_str_correctness() {
        assert!(constant_time_eq_str("abc", "abc"));
        assert!(!constant_time_eq_str("abc", "abd"));
        // Length-differing pairs must still compare unequal.
        assert!(!constant_time_eq_str("abc", "abcd"));
        assert!(!constant_time_eq_str("abcd", "abc"));
        // Empty strings: both empty equal; empty vs non-empty unequal.
        assert!(constant_time_eq_str("", ""));
        assert!(!constant_time_eq_str("", "x"));
    }
}
