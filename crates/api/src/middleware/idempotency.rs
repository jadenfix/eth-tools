//! `Idempotency-Key` middleware for write endpoints.
//!
//! Contract per plan §9.1:
//!   - The header `Idempotency-Key: <opaque>` is OPTIONAL but recommended.
//!   - When present, the response is cached for 24h keyed by
//!     `(api_key_id, request_path, client_key)`. The same key replayed
//!     within the TTL returns the cached `(status, body)` verbatim — no
//!     re-execution.
//!   - The cache lives in `idempotency_keys` (Postgres). See
//!     `crates/db::idempotency`.
//!
//! Implementation notes:
//!   - We can't run the inner handler "speculatively" and discard the result
//!     for cache misses, so the middleware reads the response body once,
//!     persists `(status, body)`, then re-emits both back to the client.
//!     The body is bounded to 1 MiB to keep a runaway handler from
//!     ballooning Postgres rows.
//!   - On cache hit we return the cached body and DO NOT call the handler.
//!     `api_key_id = None` for the dev token (see `auth.rs`).

use axum::body::{to_bytes, Body};
use axum::extract::{Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use serde_json::Value;

use crate::error::ApiError;
use crate::AppState;

/// Cache responses for 24h. Plan §9.1.
pub const TTL_SECONDS: i64 = 24 * 60 * 60;

/// Max response body the middleware will buffer + store. 1 MiB is well above
/// any realistic write-endpoint payload; oversize responses bypass the cache
/// (the request still completes but the next replay re-executes).
pub const MAX_BODY_BYTES: usize = 1024 * 1024;

const HEADER_NAME: &str = "Idempotency-Key";

fn read_key(headers: &HeaderMap) -> Option<String> {
    headers
        .get(HEADER_NAME)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.len() <= 256)
}

/// Middleware entrypoint. Mounted on every write route.
pub async fn enforce(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Result<Response, ApiError> {
    let key = match read_key(req.headers()) {
        Some(k) => k,
        None => return Ok(next.run(req).await),
    };
    let path = req.uri().path().to_string();
    // No API-key bound to the request in Phase-4 (Phase-5 attaches one via
    // request extensions). Use `None` so dev replays still cache.
    let key_hash = eth_tools_db::idempotency::hash_key(None, &path, &key);

    if let Some(cached) = eth_tools_db::idempotency::lookup(&state.pool, &key_hash).await? {
        return Ok(build_response(cached.response_status, &cached.response_body));
    }

    // Cache miss — run the handler, capture its response, cache it.
    let resp = next.run(req).await;
    let (parts, body) = resp.into_parts();
    let bytes = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(b) => b,
        Err(_) => {
            // Body too large to cache; pass through without persistence.
            return Ok(Response::from_parts(parts, Body::empty()));
        }
    };
    let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let status = parts.status.as_u16() as i32;
    // Fire-and-log the cache write; surface errors but don't fail the
    // request (the handler already produced the answer).
    if let Err(e) = eth_tools_db::idempotency::store(
        &state.pool,
        &key_hash,
        None,
        &path,
        status,
        &value,
        TTL_SECONDS,
    )
    .await
    {
        tracing::warn!(error = ?e, path = %path, "idempotency cache write failed");
    }
    Ok(Response::from_parts(parts, Body::from(bytes)))
}

fn build_response(status: i32, body: &Value) -> Response {
    let bytes = serde_json::to_vec(body).unwrap_or_default();
    let mut resp = Response::new(Body::from(bytes));
    *resp.status_mut() = StatusCode::from_u16(status as u16).unwrap_or(StatusCode::OK);
    resp.headers_mut().insert(
        axum::http::header::CONTENT_TYPE,
        axum::http::HeaderValue::from_static("application/json"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers_with(key: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(v) = key {
            h.insert(HEADER_NAME, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn read_key_happy_path() {
        let h = headers_with(Some("abc-123"));
        assert_eq!(read_key(&h), Some("abc-123".to_string()));
    }
    #[test]
    fn read_key_trims_whitespace() {
        let h = headers_with(Some("  abc  "));
        assert_eq!(read_key(&h), Some("abc".to_string()));
    }
    #[test]
    fn read_key_rejects_empty() {
        let h = headers_with(Some(""));
        assert_eq!(read_key(&h), None);
        let h = headers_with(Some("   "));
        assert_eq!(read_key(&h), None);
    }
    #[test]
    fn read_key_rejects_oversize() {
        let big = "x".repeat(300);
        let h = headers_with(Some(&big));
        assert_eq!(read_key(&h), None);
    }
    #[test]
    fn read_key_missing_header() {
        let h = headers_with(None);
        assert_eq!(read_key(&h), None);
    }
    #[test]
    fn build_response_round_trips_status_and_body() {
        let body = serde_json::json!({"data": 1});
        let r = build_response(201, &body);
        assert_eq!(r.status(), StatusCode::CREATED);
        assert_eq!(
            r.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
            "application/json"
        );
    }
}
