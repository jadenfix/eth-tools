//! Shared cron entrypoint helper. Every `api/cron/*.rs` calls `serve(...)`.
//!
//! Enforces the four guards from plan §3:
//!  1. Cron secret check (**fail-closed in production**).
//!  2. Env-aware skip (non-prod returns `{skipped: true}`).
//!  3. Dry-run via `?dryrun=1`.
//!  4. Uniform telemetry envelope.
//!
//! Note: the pg_try_advisory_lock concurrency guard is added when the
//! `crates/db` sqlx pool lands; this helper takes the request body callback so
//! that future expansion stays additive.

use std::future::Future;

use http::StatusCode;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use vercel_runtime::{Error, Response, ResponseBody};

use crate::WorkerSummary;

/// Run a cron worker. The closure receives a `dryrun: bool` and returns the
/// summary it would have written.
///
/// Generic over the request body type: production passes
/// `vercel_runtime::Request` (`http::Request<hyper::body::Incoming>`); tests
/// pass `http::Request<()>` (no real hyper connection required). The body is
/// never read here — this guard inspects headers, method, and URI only.
pub async fn serve<F, Fut, B>(
    worker_name: &'static str,
    req: http::Request<B>,
    body: F,
) -> Result<Response<ResponseBody>, Error>
where
    F: FnOnce(bool) -> Fut,
    Fut: Future<Output = Result<WorkerSummary, Error>>,
{
    let env = std::env::var("VERCEL_ENV").unwrap_or_else(|_| "development".into());
    let is_prod = env == "production";

    // 1. Cron secret. **Fail-closed in production.** Missing secret = server
    //    misconfig, not a free pass. Bootstrap allowance: outside production
    //    (preview / development) we skip the check to avoid friction when
    //    triggering workers manually from a dev shell.
    //
    // Vercel cron auto-injects `Authorization: Bearer ${CRON_SECRET}` per
    // https://vercel.com/docs/cron-jobs/manage-cron-jobs#securing-cron-jobs —
    // NOT a custom `x-vercel-cron-secret` header. The earlier draft of the plan
    // had the header name wrong; two prior deep reviews accepted it literally.
    // The cost of the mistake: every real cron tick rejected 401 the moment
    // worker bodies replace the current stubs.
    //
    // Comparison uses `subtle::ConstantTimeEq` over fixed-size SHA-256 digests
    // of both inputs. Hashing first prevents the early-return-on-length-mismatch
    // that a naive `len == len && fold-xor` exposes (an attacker could probe the
    // secret's length via timing).
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
    if !secret_ok {
        let code = if is_prod && expected.is_none() {
            "CRON_SECRET_MISCONFIGURED"
        } else {
            "INVALID_CRON_SECRET"
        };
        // Server-side tracing keeps the env + worker for ops; the wire response
        // is intentionally minimal — unauthenticated callers don't need to learn
        // `VERCEL_ENV` or the precise worker name.
        tracing::warn!(worker = worker_name, vercel_env = %env, code, "cron auth failed");
        let body = serde_json::json!({ "error": code });
        let status = if code == "CRON_SECRET_MISCONFIGURED" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::UNAUTHORIZED
        };
        return Ok(Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(ResponseBody::from(body.to_string()))?);
    }

    // 2. Env-aware skip.
    if !is_prod {
        let summary = WorkerSummary::skipped(worker_name, "non-production");
        return ok_json(&summary);
    }

    // 3. Dry-run.
    let dryrun = req
        .uri()
        .query()
        .map(|q| q.split('&').any(|kv| kv == "dryrun=1" || kv == "dryrun=true"))
        .unwrap_or(false);

    // 4. Run the body; write telemetry envelope. (Real `worker_runs` insert
    //    lands when `crates/db` sqlx is wired.)
    let mut summary = body(dryrun).await?;
    if dryrun {
        summary.dryrun = true;
    }
    ok_json(&summary)
}

fn ok_json<T: serde::Serialize>(value: &T) -> Result<Response<ResponseBody>, Error> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(ResponseBody::from(serde_json::to_string(value)?))?)
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

    // `serve()` never reads the body — `()` is the cheapest body type to
    // construct and avoids depending on `hyper::body::Incoming` (which only
    // exists on a live connection in 2.x).
    fn req_with(headers: &[(&'static str, &'static str)], query: Option<&str>) -> http::Request<()> {
        let mut builder = http::Request::builder().method("POST").uri(match query {
            Some(q) => format!("https://example.com/api/cron/test?{q}"),
            None => "https://example.com/api/cron/test".into(),
        });
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        builder.body(()).unwrap()
    }

    // Single test: env-var mutation is process-global and would race if split
    // across parallel #[test] cases.
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

        // Case 3: production, correct secret → 200 (body ran).
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer right")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 4: production, correct secret, ?dryrun=1 → body sees dryrun=true.
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

        // Case 7: secret of different length must be rejected (length-hiding
        // ct compare must still return false — correctness, not timing).
        let r = serve(
            "test_w",
            req_with(&[("authorization", "Bearer configuredXX")], None),
            |_| async { panic!("body should not run on length-mismatched secret") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 8: HTTP headers are case-insensitive AND the scheme accepts
        // lowercase "bearer" — both variants must work.
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

        // Case 9: malformed authorization header (no Bearer prefix) → 401.
        let r = serve(
            "test_w",
            req_with(&[("authorization", "configured")], None),
            |_| async { panic!("body should not run without Bearer prefix") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 10: x-vercel-cron-secret header is now IGNORED — Vercel doesn't
        // send it. Regression coverage that we don't accidentally re-add support.
        let r = serve(
            "test_w",
            req_with(&[("x-vercel-cron-secret", "configured")], None),
            |_| async { panic!("legacy x-vercel-cron-secret header must NOT auth") },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        std::env::remove_var("VERCEL_ENV");
        std::env::remove_var("CRON_SECRET");
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
