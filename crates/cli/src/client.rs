//! Thin typed wrapper over `reqwest` for the eth-tools API.
//!
//! Contract reference: `app/lib/api-types.ts` (auto-generated from the OpenAPI
//! spec). We don't share types — the CLI deserializes into local structs that
//! mirror the documented shape. Endpoints not yet implemented server-side
//! (`/agents/search`, `/manifest/{validate,hash}`, `/auth/whoami`) are wired
//! here so the CLI is ready the moment the server lands them.

use anyhow::{anyhow, Context, Result};
use reqwest::{header, Method, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::time::Duration;

pub const DEFAULT_API_URL: &str = "https://eth-tools.dev";

/// Validate that `raw` is a syntactically valid base URL the CLI is willing to
/// talk to. Returns the parsed-and-normalized string on success.
///
/// Rules:
///   * must parse with `url::Url`
///   * scheme must be `https`, OR `http` against a loopback host
///   * loopback = IPv4 `127.0.0.0/8`, IPv6 `::1`, or literal `localhost`
pub fn validate_api_url(raw: &str) -> Result<String> {
    let parsed =
        url::Url::parse(raw).with_context(|| format!("invalid --api-url {raw:?}: not a valid URL"))?;
    match parsed.scheme() {
        "https" => {}
        "http" => {
            if !is_loopback_host(&parsed) {
                return Err(anyhow!(
                    "invalid --api-url {raw:?}: plaintext http:// is only \
                     permitted for localhost (127.0.0.0/8, ::1, localhost); \
                     refusing to send bearer tokens in the clear"
                ));
            }
        }
        other => {
            return Err(anyhow!(
                "invalid --api-url {raw:?}: unsupported scheme {other:?}; expected https"
            ));
        }
    }
    Ok(parsed.to_string().trim_end_matches('/').to_string())
}

fn is_loopback_host(u: &url::Url) -> bool {
    match u.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Wire shape for API errors: `{ "error": { "code": ..., "evaluator": ..., "policy_version": ... } }`.
#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    error: ApiErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ApiErrorDetail {
    code: String,
    #[serde(default)]
    evaluator: Option<String>,
}

#[derive(Clone)]
pub struct Client {
    base: String,
    token: Option<String>,
    http: reqwest::Client,
}

impl Client {
    pub fn new(base: impl Into<String>, token: Option<String>) -> Result<Self> {
        let base = validate_api_url(&base.into())?;
        let http = reqwest::Client::builder()
            .user_agent(concat!("eth-tools-cli/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            // Bound every request: connect 10s, total 30s. Without these the
            // CLI would hang forever on a stalled TCP handshake or slow body.
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            // Refuse redirects: the CLI talks JSON to a single base URL, and a
            // same-host https→http redirect would silently downgrade the
            // Authorization header onto the wire in plaintext.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .context("build reqwest client")?;
        Ok(Self { base, token, http })
    }

    pub fn base_url(&self) -> &str {
        &self.base
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base, path)
        } else {
            format!("{}/{}", self.base, path)
        }
    }

    async fn send(&self, method: Method, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut req = self.http.request(method, self.url(path));
        if let Some(tok) = self.token.as_deref() {
            req = req.header(header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req.send().await.context("HTTP request failed")?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("read response body")?;

        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&bytes).into_owned();
            return Err(anyhow!(
                "{} {} -> {}",
                self.base,
                path,
                pretty_error(status, &body_text)
            ));
        }

        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice::<Value>(&bytes)
            .with_context(|| format!("decode JSON from {}{}", self.base, path))
    }

    // ---- High-level endpoints ----

    pub async fn health(&self) -> Result<Value> {
        self.send(Method::GET, "/api/v1/health", None).await
    }

    pub async fn whoami(&self) -> Result<Value> {
        self.send(Method::GET, "/api/v1/auth/whoami", None).await
    }

    pub async fn search_agents(&self, query: &str) -> Result<Value> {
        let body = serde_json::json!({ "query": query });
        self.send(Method::POST, "/api/v1/agents/search", Some(&body))
            .await
    }

    pub async fn inspect_agent(&self, chain: &str, agent_id: &str) -> Result<Value> {
        let path = format!("/api/v1/agents/{}/{}", chain, agent_id);
        self.send(Method::GET, &path, None).await
    }

    pub async fn manifest_validate(&self, manifest: &Value) -> Result<Value> {
        self.send(Method::POST, "/api/v1/manifest/validate", Some(manifest))
            .await
    }

    pub async fn manifest_hash(&self, manifest: &Value) -> Result<Value> {
        self.send(Method::POST, "/api/v1/manifest/hash", Some(manifest))
            .await
    }
}

fn pretty_error(status: StatusCode, body: &str) -> String {
    if let Ok(parsed) = serde_json::from_str::<ApiErrorBody>(body) {
        let suffix = parsed
            .error
            .evaluator
            .as_deref()
            .map(|e| format!(" @ {e}"))
            .unwrap_or_default();
        format!("{} ({}{})", status, parsed.error.code, suffix)
    } else if body.is_empty() {
        format!("{status} (empty body)")
    } else {
        format!("{status}: {body}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_https() {
        assert_eq!(
            validate_api_url("https://eth-tools.dev").unwrap(),
            "https://eth-tools.dev"
        );
        // Trailing slash is normalized off.
        assert_eq!(
            validate_api_url("https://eth-tools.dev/").unwrap(),
            "https://eth-tools.dev"
        );
    }

    #[test]
    fn validate_accepts_http_localhost() {
        for ok in [
            "http://127.0.0.1:8080",
            "http://localhost:3000",
            "http://[::1]:8080",
            "http://LOCALHOST",
        ] {
            assert!(validate_api_url(ok).is_ok(), "expected ok: {ok}");
        }
    }

    #[test]
    fn validate_rejects_http_remote() {
        let err = validate_api_url("http://attacker.com").unwrap_err().to_string();
        assert!(err.contains("plaintext http://"), "unexpected error: {err}");
    }

    #[test]
    fn validate_rejects_weird_schemes() {
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "ftp://x"] {
            assert!(validate_api_url(bad).is_err(), "expected reject: {bad}");
        }
    }

    #[test]
    fn validate_rejects_garbage() {
        assert!(validate_api_url("not a url").is_err());
        assert!(validate_api_url("").is_err());
    }
}
