//! Vercel function entrypoint for /api/v1/*.
//!
//! Bridges `vercel_runtime::Request` (2.x — `http::Request<hyper::body::Incoming>`)
//! to the shared `eth_tools_api::router` via `tower::Service::oneshot`, and
//! converts the response back to `http::Response<vercel_runtime::ResponseBody>`.
//!
//! Lifecycle: at cold-start we lazily build `AppState { pool }` once and
//! stash it (plus the router that holds it) in a `tokio::sync::OnceCell`.
//! Subsequent warm invocations skip the rebuild. Initialization is async
//! because `eth_tools_db::connect` is.
//!
//! Vercel routes `/api/v1/*` here via `vercel.json` rewrites
//! (`api/v1/(.*)` → `/api/v1/index`).

use axum::body::Body as AxumBody;
use http::Request as HttpRequest;
use tokio::sync::OnceCell;
use tower::ServiceExt;
use vercel_runtime::{run, Error, Request, Response, ResponseBody};

static APP: OnceCell<axum::Router> = OnceCell::const_new();

#[tokio::main]
async fn main() -> Result<(), Error> {
    tracing_subscriber::fmt()
        .with_target(false)
        .with_ansi(false)
        .json()
        .init();
    run(vercel_runtime::service_fn(handler)).await
}

async fn handler(req: Request) -> Result<Response<ResponseBody>, Error> {
    let app = APP
        .get_or_try_init(|| async {
            let url = std::env::var("DATABASE_URL")
                .map_err(|_| Error::from("DATABASE_URL must be set in production"))?;
            let pool = eth_tools_db::connect(&url)
                .await
                .map_err(|e| Error::from(format!("db connect: {e}")))?;
            Ok::<_, Error>(eth_tools_api::router(eth_tools_api::AppState::new(pool)))
        })
        .await?;

    // 2.x `Request` carries `hyper::body::Incoming`. Wrap it in `axum::body::Body`
    // (which is a generic `http_body::Body` adapter) so the bridge can stay
    // generic and the test suite can drive it with `Body::empty()` /
    // `Body::from(Vec<u8>)` — no real hyper connection needed.
    let (parts, body) = req.into_parts();
    let axum_req = HttpRequest::from_parts(parts, AxumBody::new(body));
    bridge(app, axum_req).await
}

/// Response body cap. Vercel itself enforces ~4.5 MB on Functions, but our
/// JSON envelopes are <100 KB; cap at 10 MB so a buggy handler can't OOM the
/// isolate by emitting an unbounded stream.
const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Pure conversion function — Axum request → Axum oneshot → Vercel Response.
/// Extracted so the test suite can drive it against any Router, no env-var
/// or DATABASE_URL plumbing required.
///
/// The body type is fixed to `axum::body::Body` so production (`Incoming`
/// wrapped via `Body::new`) and tests (`Body::empty()`, `Body::from(Vec<u8>)`)
/// share a single signature. The 10 MB cap still bounds collection.
async fn bridge(
    app: &axum::Router,
    req: HttpRequest<AxumBody>,
) -> Result<Response<ResponseBody>, Error> {
    // `Router::Service::Error` is `Infallible`; the empty `match` proves
    // unreachability to the compiler without ever calling `unwrap` (a panic
    // here would tear down the Vercel isolate and discard the cached
    // AppState, causing a cold-start storm on the next call). The closure
    // body has type `!` which coerces to the Ok arm's type, so there's no
    // runtime cost.
    let axum_resp = app
        .clone()
        .oneshot(req)
        .await
        .unwrap_or_else(|e: std::convert::Infallible| match e {});

    let (parts, body) = axum_resp.into_parts();
    let body_bytes = axum::body::to_bytes(body, MAX_BODY_BYTES)
        .await
        .map_err(|e| Error::from(format!("response body collect: {e}")))?;
    Ok(Response::from_parts(parts, ResponseBody::from(body_bytes)))
}

#[cfg(test)]
mod tests {
    //! Bridge round-trip against a stub Router (no DB needed) — proves the
    //! conversion contract. The full handler + AppState path is covered by
    //! crates/api's testcontainers tests.
    //!
    //! Note vs. 1.x: tests build requests with `axum::body::Body` (since 2.x's
    //! `Request<Incoming>` is unconstructible outside a real hyper connection)
    //! and read responses by collecting the `ResponseBody` through
    //! `http_body_util::BodyExt::collect`. The asserted behaviors
    //! (status, headers, body bytes) are unchanged.

    use super::*;
    use axum::routing::get;
    use http_body_util::BodyExt;

    fn stub_router() -> axum::Router {
        axum::Router::new()
            .route("/echo", get(|| async { "echoed" }))
            .route(
                "/api/v1/health",
                get(|| async { axum::Json(serde_json::json!({"status": "ok"})) }),
            )
            .fallback(|| async {
                (
                    http::StatusCode::NOT_FOUND,
                    axum::Json(serde_json::json!({"error": {"code": "ROUTE_NOT_FOUND"}})),
                )
            })
    }

    async fn collect_body(resp: Response<ResponseBody>) -> (http::response::Parts, Vec<u8>) {
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes().to_vec();
        (parts, bytes)
    }

    #[tokio::test]
    async fn bridge_round_trip_text() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/echo")
            .body(AxumBody::empty())
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        let (parts, body) = collect_body(resp).await;
        assert_eq!(parts.status, 200);
        assert_eq!(String::from_utf8(body).unwrap(), "echoed");
    }

    #[tokio::test]
    async fn bridge_round_trip_json() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/api/v1/health")
            .body(AxumBody::empty())
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        let (parts, body) = collect_body(resp).await;
        assert_eq!(parts.status, 200);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn bridge_passes_through_404_envelope() {
        let app = stub_router();
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/no-such-route")
            .body(AxumBody::empty())
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        let (parts, body) = collect_body(resp).await;
        assert_eq!(parts.status, 404);
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "ROUTE_NOT_FOUND");
    }

    /// A non-UTF-8 response body must survive the round trip byte-for-byte.
    /// In 1.x this required the `Body::Binary` variant; in 2.x `ResponseBody`
    /// is opaque bytes (no Text/Binary split), so we assert directly on the
    /// collected bytes — including the `content-type: octet-stream` header
    /// that downstreams use to distinguish binary from JSON.
    #[tokio::test]
    async fn bridge_round_trip_binary() {
        let app = axum::Router::new().route(
            "/blob",
            get(|| async {
                // 0x80 is the smallest byte that's invalid as a UTF-8 start byte.
                let bytes: Vec<u8> = vec![0xff, 0xfe, 0xfd, 0x80, 0x00, 0x01];
                axum::http::Response::builder()
                    .status(200)
                    .header("content-type", "application/octet-stream")
                    .body(axum::body::Body::from(bytes))
                    .unwrap()
            }),
        );
        let req = http::Request::builder()
            .method("GET")
            .uri("https://example.com/blob")
            .body(AxumBody::empty())
            .unwrap();
        let resp = bridge(&app, req).await.unwrap();
        let (parts, body) = collect_body(resp).await;
        assert_eq!(parts.status, 200);
        assert_eq!(
            parts.headers.get("content-type").map(|v| v.as_bytes()),
            Some(&b"application/octet-stream"[..])
        );
        assert_eq!(body, vec![0xff, 0xfe, 0xfd, 0x80, 0x00, 0x01]);
    }
}
