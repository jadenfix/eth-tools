//! Bearer-token gate for write endpoints.
//!
//! The real API-key store lives in `crates/db::api_keys` (Phase-5,
//! `feat/phase-5-api-keys`). Until that lands we honour two paths:
//!
//!   1. Any token equal to the dev token `et_test_dev` passes (so MCP and
//!      the dashboard can integrate against the spec).
//!   2. Tokens prefixed with `et_live_` are forwarded to a future
//!      `validate_api_key` call — for now we accept any 32+ char token of
//!      that shape so signed clients work.
//!
//! With the `test-bypass-auth` Cargo feature the middleware short-circuits
//! to allow — handler-only tests don't need to mint a token per case.

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use subtle::ConstantTimeEq;

use crate::error::ApiError;

/// Hard-coded dev token. Documented in README so contributors can curl write
/// endpoints from local dev without minting a real API key.
pub const DEV_TOKEN: &str = "et_test_dev";

/// Extract the bearer token from `Authorization: Bearer <token>`. Returns
/// `None` if the header is missing/malformed — caller turns that into a 401.
fn extract_bearer(req: &Request) -> Option<&str> {
    let raw = req.headers().get(axum::http::header::AUTHORIZATION)?.to_str().ok()?;
    let token = raw.strip_prefix("Bearer ")?;
    if token.is_empty() {
        return None;
    }
    Some(token)
}

/// Validate a token. Constant-time compare against the dev token; live-token
/// check is a placeholder until the API-key crate lands. Splitting this out
/// makes the test surface tiny.
fn is_valid_token(token: &str) -> bool {
    if token.as_bytes().ct_eq(DEV_TOKEN.as_bytes()).into() {
        return true;
    }
    // TODO(feat/phase-5-api-keys): swap the line below for
    // `db::api_keys::find_by_prefix_and_verify(...).await`.
    token.starts_with("et_live_") && token.len() >= 32
}

/// Axum middleware: reject when no bearer token or token fails validation.
pub async fn require_bearer(req: Request, next: Next) -> Result<Response, ApiError> {
    if cfg!(feature = "test-bypass-auth") {
        return Ok(next.run(req).await);
    }
    let token = extract_bearer(&req).ok_or(ApiError::Unauthorized)?;
    if !is_valid_token(token) {
        return Err(ApiError::Unauthorized);
    }
    Ok(next.run(req).await)
}

// `axum::middleware::from_fn` requires `Result<R, E>` where
// `R: IntoResponse`; `ApiError` already implements `IntoResponse` via
// `crate::error`, so this middleware's return type compiles as-is.

#[cfg(test)]
mod tests {
    use super::*;

    fn req_with_auth(value: Option<&str>) -> Request {
        let mut builder = http::Request::builder().uri("/");
        if let Some(v) = value {
            builder = builder.header("Authorization", v);
        }
        builder.body(axum::body::Body::empty()).unwrap()
    }

    #[test]
    fn extract_bearer_happy_path() {
        let r = req_with_auth(Some("Bearer et_test_dev"));
        assert_eq!(extract_bearer(&r), Some("et_test_dev"));
    }
    #[test]
    fn extract_bearer_missing_header() {
        let r = req_with_auth(None);
        assert_eq!(extract_bearer(&r), None);
    }
    #[test]
    fn extract_bearer_wrong_scheme() {
        let r = req_with_auth(Some("Basic et_test_dev"));
        assert_eq!(extract_bearer(&r), None);
    }
    #[test]
    fn extract_bearer_empty_token() {
        let r = req_with_auth(Some("Bearer "));
        assert_eq!(extract_bearer(&r), None);
    }
    #[test]
    fn dev_token_accepted() {
        assert!(is_valid_token(DEV_TOKEN));
    }
    #[test]
    fn live_token_shape_accepted() {
        assert!(is_valid_token(
            "et_live_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        ));
    }
    #[test]
    fn random_string_rejected() {
        assert!(!is_valid_token("hunter2"));
        assert!(!is_valid_token("et_live_short"));
    }
}
