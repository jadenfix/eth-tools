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

/// Typed error so commands can distinguish "endpoint not deployed yet" (404)
/// from a real failure. Wrapped in `anyhow::Error` via `Into`, so existing
/// command code that just `?`s the result continues to work.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({})", self.status, self.message)
    }
}

impl std::error::Error for ApiError {}

pub const DEFAULT_API_URL: &str = "https://eth-tools.dev";

/// Validate that `raw` is a syntactically valid base URL the CLI is willing to
/// talk to. Returns the parsed-and-normalized string on success.
///
/// Rules:
///   * must parse with `url::Url`
///   * scheme must be `https`, OR `http` against a loopback host
///   * loopback = IPv4 `127.0.0.0/8`, IPv6 `::1` (incl. IPv4-mapped
///     `::ffff:127.0.0.1/104`), or literal `localhost`
///   * MUST NOT contain `userinfo` (`user:pass@host`). reqwest preserves the
///     userinfo through to the wire and injects a `Basic` auth header on every
///     request — IN ADDITION to our Bearer token. A hostile `--api-url
///     https://evil:creds@eth-tools.dev` would exfiltrate the typed `creds`
///     to a trusted host with no validator firing.
pub fn validate_api_url(raw: &str) -> Result<String> {
    let parsed =
        url::Url::parse(raw).with_context(|| format!("invalid --api-url {raw:?}: not a valid URL"))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(anyhow!(
            "--api-url must not contain userinfo (user:pass@host)"
        ));
    }
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
        Some(url::Host::Ipv6(ip)) => {
            // `Ipv6Addr::is_loopback` is strict `::1` only. IPv4-mapped
            // loopback (`::ffff:127.0.0.1`) routes to 127.0.0.1 on every
            // common kernel, so a dev workflow that bracket-quotes the
            // mapped form must still pass.
            ip.is_loopback()
                || ip
                    .to_ipv4_mapped()
                    .map(|v4| v4.is_loopback())
                    .unwrap_or(false)
        }
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
            let pretty = pretty_error(status, &body_text);
            // Preserve the status code via a typed error so callers can
            // distinguish 404 / 402 / etc. via `err.downcast_ref::<ApiError>()`.
            return Err(anyhow::Error::new(ApiError {
                status,
                message: pretty.clone(),
            })
            .context(format!("{} {} -> {}", self.base, path, pretty)));
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

    /// `POST /api/v1/manifest/generate` — server-side manifest construction
    /// (route lands with phase-7 server work). Body is a free-form JSON object
    /// of registration inputs (name, description, skills, services).
    pub async fn manifest_generate(&self, inputs: &Value) -> Result<Value> {
        self.send(Method::POST, "/api/v1/manifest/generate", Some(inputs))
            .await
    }

    /// `POST /api/v1/invoke` — execute a tool against a registered agent.
    /// `body` is `{ "agent": "<chain>/<id>", "input": <json>, ...optional fields }`.
    /// On 402 (Payment Required) callers should inspect the error and surface
    /// the x402 challenge to the user.
    pub async fn invoke(&self, body: &Value) -> Result<Value> {
        self.send(Method::POST, "/api/v1/invoke", Some(body)).await
    }

    /// `POST /api/v1/invoke` with an extra `X-Payment` header carrying an
    /// EIP-3009 USDC authorization. Used after a 402 challenge.
    pub async fn invoke_with_payment(&self, body: &Value, x_payment: &str) -> Result<Value> {
        let mut req = self
            .http
            .request(Method::POST, self.url("/api/v1/invoke"))
            .header("X-Payment", x_payment)
            .json(body);
        if let Some(tok) = self.token.as_deref() {
            req = req.header(header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        let resp = req.send().await.context("HTTP request failed")?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("read response body")?;
        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&bytes).into_owned();
            let pretty = pretty_error(status, &body_text);
            return Err(anyhow::Error::new(ApiError {
                status,
                message: pretty.clone(),
            })
            .context(format!("{} /api/v1/invoke -> {}", self.base, pretty)));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice::<Value>(&bytes).context("decode invoke response")
    }

    /// `GET /api/v1/agents?chain=<chain>&since=<cursor>&limit=...` — used by
    /// `watch` to poll for new agents until a real SSE backend lands.
    pub async fn list_agents_since(
        &self,
        chain: &str,
        since: Option<&str>,
    ) -> Result<Value> {
        let mut q: Vec<(&str, &str)> = vec![("chain", chain), ("limit", "100")];
        if let Some(c) = since {
            q.push(("since", c));
        }
        let url = reqwest::Url::parse_with_params(&self.url("/api/v1/agents"), &q)
            .context("build /agents URL")?;
        let mut req = self.http.request(Method::GET, url);
        if let Some(tok) = self.token.as_deref() {
            req = req.header(header::AUTHORIZATION, format!("Bearer {tok}"));
        }
        let resp = req.send().await.context("HTTP request failed")?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("read response body")?;
        if !status.is_success() {
            let body_text = String::from_utf8_lossy(&bytes).into_owned();
            let pretty = pretty_error(status, &body_text);
            return Err(anyhow::Error::new(ApiError {
                status,
                message: pretty.clone(),
            })
            .context(format!("{} /api/v1/agents -> {}", self.base, pretty)));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice::<Value>(&bytes).context("decode /agents response")
    }

    /// `GET /api/v1/workers/status` — lightweight endpoint (not yet deployed).
    /// Returns the underlying error verbatim; the command layer detects 404
    /// and surfaces a friendly "not yet deployed" message.
    pub async fn workers_status(&self) -> Result<Value> {
        self.send(Method::GET, "/api/v1/workers/status", None).await
    }

    /// `GET /api/v1/wallet/status` — same caveat as `workers_status`.
    pub async fn wallet_status(&self) -> Result<Value> {
        self.send(Method::GET, "/api/v1/wallet/status", None).await
    }

    /// Fetch an arbitrary URL (agent-card JSON). Used by `mcp from-card`.
    /// Bypasses the API base URL but still routes through the hardened
    /// reqwest client (timeouts, no redirects, no userinfo). We allow
    /// plain `http://` here (unlike `validate_api_url`) because no
    /// Authorization header is ever attached — the worst case is a
    /// content-MITM on the agent card itself, which the user has opted
    /// into by pasting a non-https URL.
    pub async fn fetch_agent_card(&self, url: &str) -> Result<Value> {
        let parsed = url::Url::parse(url).with_context(|| format!("invalid URL {url:?}"))?;
        if !matches!(parsed.scheme(), "https" | "http") {
            return Err(anyhow!(
                "unsupported scheme {:?}; expected http(s)",
                parsed.scheme()
            ));
        }
        if !parsed.username().is_empty() || parsed.password().is_some() {
            return Err(anyhow!("URL must not contain userinfo"));
        }
        let resp = self
            .http
            .get(parsed)
            .send()
            .await
            .context("HTTP request failed")?;
        let status = resp.status();
        let bytes = resp.bytes().await.context("read response body")?;
        if !status.is_success() {
            return Err(anyhow!(
                "GET {url} -> {}",
                pretty_error(status, &String::from_utf8_lossy(&bytes))
            ));
        }
        serde_json::from_slice::<Value>(&bytes)
            .with_context(|| format!("decode JSON from {url}"))
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

    #[test]
    fn validate_rejects_userinfo() {
        // All three forms must be rejected — reqwest would otherwise inject a
        // `Basic` header constructed from the userinfo on every request, in
        // addition to our Bearer token, silently exfiltrating whatever the
        // user pasted between `https://` and `@`.
        for bad in [
            "https://user@eth-tools.dev",
            "https://user:pass@eth-tools.dev",
            "https://:pass@eth-tools.dev",
        ] {
            let err = validate_api_url(bad).unwrap_err().to_string();
            assert!(
                err.contains("must not contain userinfo"),
                "expected userinfo rejection for {bad}, got: {err}"
            );
        }
        // Sanity: the bare host still passes.
        assert!(validate_api_url("https://eth-tools.dev").is_ok());
    }

    #[test]
    fn validate_accepts_ipv4_mapped_loopback() {
        // IPv4-mapped IPv6 loopback (`::ffff:127.0.0.1`) routes to 127.0.0.1
        // on every common kernel — dev workflows that bracket-quote the
        // mapped form must work.
        for ok in [
            "http://[::ffff:127.0.0.1]",
            "http://[::ffff:127.0.0.1]:3000",
        ] {
            assert!(validate_api_url(ok).is_ok(), "expected ok: {ok}");
        }
    }

    #[test]
    fn validate_rejects_ipv4_mapped_remote() {
        // `::ffff:8.8.8.8` is NOT loopback; combined with http it must be
        // rejected by the https-only check.
        let err = validate_api_url("http://[::ffff:8.8.8.8]")
            .unwrap_err()
            .to_string();
        assert!(err.contains("plaintext http://"), "unexpected error: {err}");
    }
}
