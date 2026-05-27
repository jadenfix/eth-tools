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

/// In-memory token store with three possible states:
///   - `Configured(token)`  — bearer-token gate is live.
///   - `Open`               — auth disabled (dev/test). Logged at boot.
///   - `Misconfigured`      — `VERCEL_ENV=production` but no token was
///     supplied. Every request fails closed with HTTP 500
///     `MCP_BEARER_TOKEN_MISCONFIGURED`, mirroring the cron-secret pattern
///     in `crates/workers/src/cron.rs`.
#[derive(Debug, Clone, Default)]
pub struct AuthConfig {
    state: AuthState,
}

#[derive(Debug, Clone, Default)]
enum AuthState {
    Configured(String),
    #[default]
    Open,
    Misconfigured,
}

impl AuthConfig {
    /// Read `MCP_BEARER_TOKEN` + `VERCEL_ENV` from the environment.
    ///
    /// - In production (`VERCEL_ENV=production`), a missing or empty
    ///   token is **fail-closed**: every request returns HTTP 500 with
    ///   `MCP_BEARER_TOKEN_MISCONFIGURED`. This mirrors the cron-secret
    ///   guard in `crates/workers/src/cron.rs`.
    /// - In non-production (`development`, `preview`, unset), a missing
    ///   token disables auth and logs a loud `tracing::warn!` at boot so
    ///   prod can't accidentally ship without it.
    pub fn from_env() -> Self {
        let token = std::env::var("MCP_BEARER_TOKEN").ok();
        let vercel_env = std::env::var("VERCEL_ENV").ok();
        Self::from_values(vercel_env.as_deref(), token.as_deref())
    }

    /// Pure variant of [`from_env`] for unit-testing without env mutation.
    /// `token == Some("")` is treated identically to `None` per the
    /// `.env.example` ship default.
    pub fn from_values(vercel_env: Option<&str>, token: Option<&str>) -> Self {
        let is_prod = vercel_env == Some("production");
        match token {
            Some(t) if !t.is_empty() => Self {
                state: AuthState::Configured(t.to_string()),
            },
            _ if is_prod => {
                tracing::error!(
                    "MCP_BEARER_TOKEN missing or empty in production — \
                     MCP endpoint will fail-closed with HTTP 500 \
                     MCP_BEARER_TOKEN_MISCONFIGURED on every request."
                );
                Self {
                    state: AuthState::Misconfigured,
                }
            }
            _ => {
                tracing::warn!(
                    "MCP_BEARER_TOKEN unset — MCP endpoint is OPEN. \
                     Set this env var in production or every request will be served unauthenticated."
                );
                Self {
                    state: AuthState::Open,
                }
            }
        }
    }

    /// Test-only constructor. Hidden behind `#[cfg(test)]` callers — there's
    /// no `cfg(test)` gate here because the integration test in
    /// `tests/integration.rs` is a separate crate and needs to call it.
    pub fn with_token(token: impl Into<String>) -> Self {
        Self {
            state: AuthState::Configured(token.into()),
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
    let expected = match &cfg.state {
        // Production was deployed without the bearer token — fail-closed
        // with HTTP 500. Mirrors `CRON_SECRET_MISCONFIGURED` in
        // `crates/workers/src/cron.rs`. A loud boot-time `tracing::error!`
        // is emitted in `from_values`, but operators may not notice; this
        // request-time guard guarantees no traffic is served open in prod.
        AuthState::Misconfigured => return misconfigured(),
        // Auth disabled in dev/test; pass through. The boot-time warning
        // is the safety net.
        AuthState::Open => return next.run(req).await,
        AuthState::Configured(t) => t.as_str(),
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

fn misconfigured() -> Response {
    let body = json!({
        "error": {
            "code": "MCP_BEARER_TOKEN_MISCONFIGURED",
            "policy_version": "v1",
            "evaluator": "mcp.auth",
            "override_hint": "set MCP_BEARER_TOKEN in the Vercel production environment"
        }
    });
    (StatusCode::INTERNAL_SERVER_ERROR, axum::Json(body)).into_response()
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
    fn from_values_dev_with_empty_token_opens() {
        // Empty token in dev = same as missing = auth disabled with a warn.
        let c = AuthConfig::from_values(Some("development"), Some(""));
        assert!(matches!(c.state, AuthState::Open));
    }

    #[test]
    fn from_values_dev_with_token_configures() {
        let c = AuthConfig::from_values(Some("development"), Some("hunter2"));
        match c.state {
            AuthState::Configured(ref t) => assert_eq!(t, "hunter2"),
            other => panic!("expected Configured, got {other:?}"),
        }
    }

    /// H1 (Phase-5 MCP deep review): production with a missing token
    /// must fail closed at request time, not silently disable auth.
    #[test]
    fn auth_fails_closed_in_production_on_missing_token() {
        let c = AuthConfig::from_values(Some("production"), None);
        assert!(
            matches!(c.state, AuthState::Misconfigured),
            "prod + missing token must be Misconfigured, got {:?}",
            c.state
        );
    }

    /// H1 (Phase-5 MCP deep review): production with the
    /// `.env.example` ship default (`MCP_BEARER_TOKEN=`, empty string)
    /// must also fail closed — the empty value must not be treated as
    /// "auth disabled".
    #[test]
    fn auth_fails_closed_in_production_on_empty_token() {
        let c = AuthConfig::from_values(Some("production"), Some(""));
        assert!(
            matches!(c.state, AuthState::Misconfigured),
            "prod + empty token must be Misconfigured, got {:?}",
            c.state
        );
    }

    /// H1 (Phase-5 MCP deep review): non-production (including unset
    /// `VERCEL_ENV`) keeps the dev-friendly Open-with-warning behavior.
    #[test]
    fn auth_open_in_dev_on_missing_token_logs_warn() {
        // Both unset-`VERCEL_ENV` and explicit `development` must Open.
        let c1 = AuthConfig::from_values(None, None);
        let c2 = AuthConfig::from_values(Some("development"), None);
        let c3 = AuthConfig::from_values(Some("preview"), None);
        assert!(matches!(c1.state, AuthState::Open));
        assert!(matches!(c2.state, AuthState::Open));
        assert!(matches!(c3.state, AuthState::Open));
    }

    /// Mirrors the cron-secret request-time guard: a `Misconfigured`
    /// AuthConfig must serve every request with HTTP 500.
    #[tokio::test]
    async fn middleware_returns_500_when_misconfigured() {
        use axum::body::Body;
        use axum::Router;
        use http::Request;
        use tower::ServiceExt;

        let cfg = Arc::new(AuthConfig::from_values(Some("production"), None));
        let app = Router::new()
            .route("/", axum::routing::any(|| async { "should not run" }))
            .layer(axum::middleware::from_fn_with_state(cfg, bearer_token));

        let req = Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", "Bearer anything")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
