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
use vercel_runtime::{Body, Error, Request, Response, StatusCode};

use crate::WorkerSummary;

/// Run a cron worker. The closure receives a `dryrun: bool` and returns the
/// summary it would have written.
pub async fn serve<F, Fut>(worker_name: &'static str, req: Request, body: F) -> Result<Response<Body>, Error>
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
    let provided = req.headers().get("x-vercel-cron-secret");
    let expected = std::env::var("CRON_SECRET").ok();
    let secret_ok = match (expected.as_deref(), provided) {
        (Some(exp), Some(got)) => {
            // Constant-time comparison to avoid leaking secret length via timing.
            let got_bytes = got.as_bytes();
            let exp_bytes = exp.as_bytes();
            got_bytes.len() == exp_bytes.len()
                && got_bytes
                    .iter()
                    .zip(exp_bytes)
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
        }
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
        let body = serde_json::json!({
            "error": code,
            "worker": worker_name,
            "vercel_env": env,
        });
        let status = if code == "CRON_SECRET_MISCONFIGURED" {
            StatusCode::INTERNAL_SERVER_ERROR
        } else {
            StatusCode::UNAUTHORIZED
        };
        return Ok(Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Body::Text(body.to_string()))?);
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

fn ok_json<T: serde::Serialize>(value: &T) -> Result<Response<Body>, Error> {
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("content-type", "application/json")
        .body(Body::Text(serde_json::to_string(value)?))?)
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
            req_with(&[("x-vercel-cron-secret", "wrong")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);

        // Case 3: production, correct secret → 200 (body ran).
        let r = serve(
            "test_w",
            req_with(&[("x-vercel-cron-secret", "right")], None),
            |_| async { Ok(WorkerSummary::ok("test_w")) },
        )
        .await
        .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        // Case 4: production, correct secret, ?dryrun=1 → body sees dryrun=true.
        let r = serve(
            "test_w",
            req_with(&[("x-vercel-cron-secret", "right")], Some("dryrun=1")),
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

        std::env::remove_var("VERCEL_ENV");
    }
}
