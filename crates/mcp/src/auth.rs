//! Bearer-token auth gate for the MCP endpoint.
//!
//! **Local shim**, not a permanent home: the plan calls for a shared
//! `crates/api::auth` module fronting both the REST API and the MCP server
//! with OAuth-protected-resource (RFC 9728) + Dynamic Client Registration
//! (RFC 7591). PR #2 was supposed to ship that crate but didn't, so this
//! file implements the minimum needed to satisfy plan §8.3 for Phase-5:
//! a single static bearer token in `MCP_BEARER_TOKEN`.
//!
//! TODO(phase-5.5): replace this entire file with
//! `eth_tools_api::auth::bearer_token` once the shared module lands. The
//! axum middleware signature should be source-compatible.

use axum::{
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::sync::Arc;
use subtle_constant_time::ct_eq;

/// In-memory token store. Empty token = auth disabled (test/dev only —
/// the constructor logs a warning so this isn't silently shipped).
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    expected: Option<String>,
}

impl AuthConfig {
    /// Read `MCP_BEARER_TOKEN` from the environment. If unset, auth is
    /// **disabled** — useful for local dev and the integration test but a
    /// loud `tracing::warn!` fires at boot so prod can't accidentally ship
    /// without it.
    pub fn from_env() -> Self {
        match std::env::var("MCP_BEARER_TOKEN") {
            Ok(v) if !v.is_empty() => Self { expected: Some(v) },
            _ => {
                tracing::warn!(
                    "MCP_BEARER_TOKEN unset — MCP endpoint is OPEN. \
                     Set this env var in production or every request will be served unauthenticated."
                );
                Self { expected: None }
            }
        }
    }

    /// Test-only constructor. Hidden behind `#[cfg(test)]` callers — there's
    /// no `cfg(test)` gate here because the integration test in
    /// `tests/integration.rs` is a separate crate and needs to call it.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            expected: Some(token.into()),
        }
    }
}

/// Axum middleware: validate `Authorization: Bearer <token>` against the
/// configured token (constant-time compare).
///
/// Returns the same JSON envelope shape as `crates/api::error::ApiError` so
/// MCP clients and the REST API have a consistent error contract.
pub async fn bearer_token(
    State(cfg): State<Arc<AuthConfig>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    // No expected token → auth disabled, pass through. The boot-time warning
    // is the safety net.
    let Some(expected) = cfg.expected.as_deref() else {
        return next.run(req).await;
    };

    let supplied = req
        .headers()
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(str::trim);

    let ok = match supplied {
        Some(tok) => ct_eq(tok.as_bytes(), expected.as_bytes()),
        None => false,
    };

    if ok {
        next.run(req).await
    } else {
        unauthorized()
    }
}

fn unauthorized() -> Response {
    let body = json!({
        "error": {
            "code": "UNAUTHORIZED",
            "policy_version": "v1",
            "evaluator": "mcp.auth",
            "override_hint": "supply `Authorization: Bearer <token>` (see /.well-known/oauth-protected-resource)"
        }
    });
    (
        StatusCode::UNAUTHORIZED,
        [
            // RFC 6750 §3 — advertise the realm + supported auth methods so
            // a compliant MCP client can prompt for credentials.
            (
                axum::http::header::WWW_AUTHENTICATE,
                "Bearer realm=\"eth-tools-mcp\"",
            ),
        ],
        axum::Json(body),
    )
        .into_response()
}

/// Tiny `subtle`-style constant-time byte compare. Avoids pulling
/// `subtle` (already a workspace dep but with a different feature set) by
/// inlining the four-line implementation. The compiler is *not* allowed to
/// short-circuit `|=` over a u8, which is the whole guarantee we need.
mod subtle_constant_time {
    pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        let mut diff: u8 = 0;
        for (x, y) in a.iter().zip(b.iter()) {
            diff |= x ^ y;
        }
        diff == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_matches_string_eq_for_equal_inputs() {
        assert!(subtle_constant_time::ct_eq(b"hello", b"hello"));
    }

    #[test]
    fn ct_eq_rejects_different_lengths() {
        assert!(!subtle_constant_time::ct_eq(b"hello", b"hellox"));
        assert!(!subtle_constant_time::ct_eq(b"", b"x"));
    }

    #[test]
    fn ct_eq_rejects_differing_bytes() {
        assert!(!subtle_constant_time::ct_eq(b"foobar", b"foobax"));
    }

    #[test]
    fn config_from_env_with_empty_token_disables() {
        // SAFETY: env mutation only inside this test. Tests in this module
        // run on a single thread when feature-gated with `--test-threads=1`,
        // but the only contract that matters is the value we observe.
        unsafe { std::env::set_var("MCP_BEARER_TOKEN", "") };
        let c = AuthConfig::from_env();
        assert!(c.expected.is_none());
        unsafe { std::env::remove_var("MCP_BEARER_TOKEN") };
    }
}
