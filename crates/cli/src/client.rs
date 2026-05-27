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

pub const DEFAULT_API_URL: &str = "https://eth-tools.dev";

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
        let http = reqwest::Client::builder()
            .user_agent(concat!("eth-tools-cli/", env!("CARGO_PKG_VERSION")))
            .gzip(true)
            .build()
            .context("build reqwest client")?;
        Ok(Self {
            base: base.into().trim_end_matches('/').to_string(),
            token,
            http,
        })
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
