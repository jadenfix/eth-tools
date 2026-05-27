//! Bearer-token verification + Axum middleware (Phase-5).
//!
//! Two surfaces:
//!   - `verify_bearer(token, pool, redis)` — the testable verification core.
//!     Looks the token up by its 11-char prefix, bcrypt-compares against the
//!     stored hash, returns `KeyClaims` on success.
//!   - `bearer_required(...)` — Axum `from_fn_with_state` middleware that
//!     enforces a valid `Authorization: Bearer …` header on write endpoints
//!     and injects `KeyClaims` into request extensions.
//!
//! Cache: optional Upstash Redis (REST). 60s TTL on positive hits keyed by
//! `apikey:<prefix>`. Misses fall through to Postgres + bcrypt verify. The
//! cache stores only `(id, github_user_id, github_login, scopes)` — never the
//! plaintext or hash.
//!
//! Internal admin surface (issuing/revoking keys from the dashboard) uses a
//! separate `X-Internal-Secret` shared with the Next.js layer — see
//! [`require_internal`].

use axum::extract::{Request, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::keys::PREFIX_LEN;
use crate::AppState;

/// Claims attached to an authenticated request. Available to handlers via
/// `request.extensions().get::<KeyClaims>()` or the typed `axum::extract`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct KeyClaims {
    pub key_id: Uuid,
    pub github_user_id: i64,
    pub github_login: String,
    pub scopes: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("missing or malformed Authorization header")]
    MissingBearer,
    #[error("invalid or revoked api key")]
    Invalid,
    #[error("internal")]
    Internal,
}

impl AuthError {
    fn status(&self) -> StatusCode {
        match self {
            AuthError::MissingBearer | AuthError::Invalid => StatusCode::UNAUTHORIZED,
            AuthError::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
    fn code(&self) -> &'static str {
        match self {
            AuthError::MissingBearer => "UNAUTHENTICATED",
            AuthError::Invalid => "INVALID_API_KEY",
            AuthError::Internal => "INTERNAL",
        }
    }
    fn hint(&self) -> &'static str {
        match self {
            AuthError::MissingBearer => {
                "send Authorization: Bearer et_live_xxx (issue at /dashboard/keys)"
            }
            AuthError::Invalid => {
                "key is revoked, mistyped, or unknown — issue a new one at /dashboard/keys"
            }
            AuthError::Internal => "transient — retry; persistent → file an issue",
        }
    }
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        if matches!(self, AuthError::Internal) {
            tracing::error!("auth internal error");
        }
        let body = json!({
            "error": {
                "code": self.code(),
                "policy_version": "v1",
                "evaluator": "api.auth",
                "override_hint": self.hint(),
            }
        });
        (self.status(), Json(body)).into_response()
    }
}

/// Minimal Upstash REST client. The Upstash REST protocol is plain HTTPS
/// with a Bearer token and a `[CMD, arg1, arg2, ...]` JSON body. We use that
/// directly so we don't pull in the full `redis` crate for two methods.
///
/// All operations are best-effort: a network failure here must NEVER 500 an
/// authenticated request — the caller falls through to the DB lookup.
#[derive(Clone)]
pub struct RedisClient {
    inner: Arc<RedisInner>,
}

struct RedisInner {
    url: String,
    token: String,
    http: reqwest::Client,
}

impl RedisClient {
    /// Build from `UPSTASH_REDIS_REST_URL` + `UPSTASH_REDIS_REST_TOKEN`. Returns
    /// `None` if either is missing — the rest of the system handles this
    /// gracefully by skipping cache and going straight to Postgres.
    pub fn from_env() -> Option<Self> {
        let url = std::env::var("UPSTASH_REDIS_REST_URL").ok()?;
        let token = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok()?;
        if url.is_empty() || token.is_empty() {
            return None;
        }
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(500))
            .build()
            .ok()?;
        Some(Self {
            inner: Arc::new(RedisInner { url, token, http }),
        })
    }

    /// GET the cached `KeyClaims` JSON; returns `None` on miss / error.
    pub async fn get_claims(&self, prefix: &str) -> Option<KeyClaims> {
        let key = format!("apikey:{prefix}");
        let resp = self
            .inner
            .http
            .post(&self.inner.url)
            .bearer_auth(&self.inner.token)
            .json(&json!(["GET", key]))
            .send()
            .await
            .ok()?;
        if !resp.status().is_success() {
            return None;
        }
        let v: serde_json::Value = resp.json().await.ok()?;
        let s = v.get("result")?.as_str()?;
        serde_json::from_str(s).ok()
    }

    /// SETEX cached claims for `ttl_seconds`. Fire-and-forget; errors logged
    /// at debug to avoid log floods on Upstash hiccups.
    pub async fn set_claims(&self, prefix: &str, claims: &KeyClaims, ttl_seconds: u64) {
        let key = format!("apikey:{prefix}");
        let val = match serde_json::to_string(claims) {
            Ok(s) => s,
            Err(_) => return,
        };
        let body = json!(["SET", key, val, "EX", ttl_seconds.to_string()]);
        let _ = self
            .inner
            .http
            .post(&self.inner.url)
            .bearer_auth(&self.inner.token)
            .json(&body)
            .send()
            .await
            .map(|r| {
                if !r.status().is_success() {
                    tracing::debug!(status = %r.status(), "redis set non-2xx");
                }
            });
    }

    /// DEL the cached entry. Called on revoke so a freshly-revoked key stops
    /// validating immediately instead of waiting out the 60s TTL.
    pub async fn del(&self, prefix: &str) {
        let key = format!("apikey:{prefix}");
        let _ = self
            .inner
            .http
            .post(&self.inner.url)
            .bearer_auth(&self.inner.token)
            .json(&json!(["DEL", key]))
            .send()
            .await;
    }
}

/// Verification cache TTL. 60s matches the spec; long enough to deflect
/// bcrypt cost on hot keys, short enough that revokes (which DEL the entry)
/// don't need to wait long even if Upstash is briefly unreachable.
pub const CACHE_TTL_SECS: u64 = 60;

/// Verify a candidate bearer token against the database.
///
/// 1. Slice the 11-char prefix.
/// 2. Cache lookup (`apikey:<prefix>`). Hit → done.
/// 3. DB lookup by prefix. Miss → `Invalid`.
/// 4. `bcrypt::verify(token, &row.key_hash)` on a blocking thread — bcrypt is
///    constant-time wrt the secret (see Keats/rust-bcrypt). Mismatch → `Invalid`.
/// 5. Fire-and-forget `UPDATE api_keys SET last_used_at = NOW()` so an
///    overloaded DB write doesn't block the read path.
/// 6. Populate cache for `CACHE_TTL_SECS`. Return claims.
pub async fn verify_bearer(
    token: &str,
    pool: &sqlx::PgPool,
    redis: Option<&RedisClient>,
) -> Result<KeyClaims, AuthError> {
    // Plaintext is base62 ASCII by construction; reject anything else up
    // front so the byte-slice below can't panic on a multi-byte char
    // landing inside the 11-byte prefix.
    if !token.is_ascii() || token.len() < PREFIX_LEN || !token.starts_with("et_") {
        return Err(AuthError::Invalid);
    }
    let prefix = &token[..PREFIX_LEN];

    if let Some(r) = redis {
        if let Some(claims) = r.get_claims(prefix).await {
            return Ok(claims);
        }
    }

    let row = eth_tools_db::api_keys::lookup_by_prefix(pool, prefix)
        .await
        .map_err(|e| {
            tracing::error!(error = ?e, "db error during bearer verify");
            AuthError::Internal
        })?
        .ok_or(AuthError::Invalid)?;

    // bcrypt::verify on the runtime would block for ~100ms.
    let token_owned = token.to_string();
    let hash_owned = row.key_hash.clone();
    let ok = tokio::task::spawn_blocking(move || bcrypt::verify(&token_owned, &hash_owned))
        .await
        .map_err(|_| AuthError::Internal)?
        .map_err(|_| AuthError::Internal)?;
    if !ok {
        return Err(AuthError::Invalid);
    }

    let claims = KeyClaims {
        key_id: row.id,
        github_user_id: row.github_user_id,
        github_login: row.github_login,
        scopes: row.scopes,
    };

    // Fire-and-forget last_used_at bump. We deliberately drop the JoinHandle.
    let pool2 = pool.clone();
    let id = claims.key_id;
    tokio::spawn(async move {
        if let Err(e) = eth_tools_db::api_keys::touch_last_used(&pool2, id).await {
            tracing::debug!(error = ?e, "touch_last_used failed");
        }
    });

    if let Some(r) = redis {
        r.set_claims(prefix, &claims, CACHE_TTL_SECS).await;
    }

    Ok(claims)
}

/// Pull `Authorization: Bearer …` out of headers and trim the prefix.
fn extract_bearer(headers: &HeaderMap) -> Result<&str, AuthError> {
    let h = headers
        .get(header::AUTHORIZATION)
        .ok_or(AuthError::MissingBearer)?
        .to_str()
        .map_err(|_| AuthError::MissingBearer)?;
    let token = h.strip_prefix("Bearer ").ok_or(AuthError::MissingBearer)?;
    if token.is_empty() {
        return Err(AuthError::MissingBearer);
    }
    Ok(token.trim())
}

/// Axum middleware. Apply via `.route_layer(from_fn_with_state(state, bearer_required))`
/// on write endpoints. Successful auth injects `KeyClaims` into
/// `request.extensions_mut()`.
pub async fn bearer_required(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let token = extract_bearer(req.headers())?.to_string();
    let claims = verify_bearer(&token, &state.pool, state.redis.as_ref()).await?;
    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}

/// Internal-only gate for the dashboard → Rust admin endpoints
/// (POST/GET/DELETE `/api/v1/keys`). Auth.js v5 sessions are JWE-encrypted
/// (A256GCM); reimplementing that decrypt path in Rust would be ~200 LOC of
/// crypto. Instead, the Next.js Route Handler verifies the session (one call
/// to `auth()`), then forwards to Rust with:
///   - `X-Internal-Secret`: shared HMAC-comparable secret
///   - `X-Github-User-Id`: numeric user id from the session
///   - `X-Github-Login`: github login from the session
///
/// In production the secret MUST be set; missing-or-empty → 503. In dev we
/// allow an unset secret (`INTERNAL_API_SECRET=""` is treated as misconfigured
/// only in prod) so `crates/dev-server` can be exercised directly with curl.
pub async fn require_internal(
    mut req: Request,
    next: Next,
) -> Result<Response, AuthError> {
    let expected = std::env::var("INTERNAL_API_SECRET").unwrap_or_default();
    let prod = std::env::var("VERCEL_ENV").as_deref() == Ok("production");

    if expected.is_empty() {
        if prod {
            tracing::error!("INTERNAL_API_SECRET unset in production — refusing");
            return Err(AuthError::Internal);
        }
        // Dev: skip the secret check entirely so curl-against-dev-server
        // works. user-id / login headers are still required.
    } else {
        let got = req
            .headers()
            .get("x-internal-secret")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        if !constant_time_eq(got.as_bytes(), expected.as_bytes()) {
            return Err(AuthError::Invalid);
        }
    }

    let user_id: i64 = req
        .headers()
        .get("x-github-user-id")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse().ok())
        .ok_or(AuthError::MissingBearer)?;
    let login: String = req
        .headers()
        .get("x-github-login")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .ok_or(AuthError::MissingBearer)?;

    req.extensions_mut().insert(InternalCaller { user_id, login });
    Ok(next.run(req).await)
}

/// Identity passed by the Next.js admin layer. Trusted only because
/// `require_internal` validated `X-Internal-Secret` first.
#[derive(Clone, Debug)]
pub struct InternalCaller {
    pub user_id: i64,
    pub login: String,
}

/// Length-checked constant-time byte compare. We don't want to leak the
/// shared secret's length via a short-circuit `==`.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn extract_bearer_happy_path() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer et_test_abc"));
        assert_eq!(extract_bearer(&h).unwrap(), "et_test_abc");
    }

    #[test]
    fn extract_bearer_missing() {
        let h = HeaderMap::new();
        assert!(matches!(extract_bearer(&h), Err(AuthError::MissingBearer)));
    }

    #[test]
    fn extract_bearer_wrong_scheme() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Basic dXNlcjpwdw=="));
        assert!(matches!(extract_bearer(&h), Err(AuthError::MissingBearer)));
    }

    #[test]
    fn extract_bearer_empty_token() {
        let mut h = HeaderMap::new();
        h.insert(header::AUTHORIZATION, HeaderValue::from_static("Bearer "));
        assert!(matches!(extract_bearer(&h), Err(AuthError::MissingBearer)));
    }

    #[test]
    fn ct_eq_basics() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[tokio::test]
    async fn verify_rejects_obviously_bad_tokens() {
        // Drive the shape-gate without touching the DB: a lazy pool never
        // connects unless a query runs, and the bad-shape branch short-
        // circuits before any query.
        let pool = sqlx::PgPool::connect_lazy("postgres://x:x@127.0.0.1/x").unwrap();
        assert!(matches!(
            verify_bearer("not-an-et-key", &pool, None).await,
            Err(AuthError::Invalid)
        ));
        assert!(matches!(
            verify_bearer("et_short", &pool, None).await,
            Err(AuthError::Invalid)
        ));
    }
}
