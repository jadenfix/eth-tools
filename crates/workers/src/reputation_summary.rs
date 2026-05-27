//! Upstash KV cache for per-agent reputation summaries (W4, plan §3 row).
//!
//! W4 recomputes a rolling summary `(count, mean_value, mode_decimals)` per
//! agent after each scan tick. The MCP `read_feedback` tool and the
//! dashboard pull that summary often enough that hitting Postgres every
//! time would be wasteful — we shove a 5-minute-TTL copy into Upstash KV
//! keyed `rep:summary:{chain_id}:{agent_id}` so reads stay cheap.
//!
//! ## Best-effort, never-fatal
//!
//! Cache write failures (network, auth, Upstash down) MUST NOT fail the
//! worker — the canonical data lives in Postgres. We log at `debug` and
//! return Ok(()).
//!
//! ## Env vars
//!
//! - `UPSTASH_REDIS_REST_URL` — base URL (e.g. `https://….upstash.io`).
//! - `UPSTASH_REDIS_REST_TOKEN` — bearer token.
//!
//! Both unset → caching is disabled (dev/CI default); the worker still
//! upserts to Postgres.

use std::time::Duration;

use eth_tools_db::feedback::AgentSummary;
use serde_json::json;

const TTL_SECONDS: u64 = 300;

/// HTTP timeout for the Upstash REST roundtrip. Short on purpose — a slow
/// cache must not gate the worker's per-tick budget.
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);

/// Build the canonical cache key for a `(chain_id, agent_id)` summary. Keep
/// agent_id as base-10 string to match how MCP / CLI users hit it.
pub fn cache_key(chain_id: i64, agent_id: &bigdecimal::BigDecimal) -> String {
    format!("rep:summary:{chain_id}:{agent_id}")
}

/// Configuration sourced from env. `None` ⇒ caching disabled.
pub struct UpstashConfig {
    pub base_url: String,
    pub token: String,
}

impl UpstashConfig {
    /// Read both env vars; return `None` if either is missing or empty.
    pub fn from_env() -> Option<Self> {
        let base_url = std::env::var("UPSTASH_REDIS_REST_URL").ok()?;
        let token = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok()?;
        if base_url.is_empty() || token.is_empty() {
            return None;
        }
        Some(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        })
    }
}

/// Push a summary to Upstash with [`TTL_SECONDS`] TTL. Returns Ok(()) on
/// any outcome a worker can safely ignore (config absent, cache failed);
/// Err only on programmer error (serializer panic, shouldn't happen).
///
/// Body is encoded as JSON; SETEX is the Upstash REST verb that takes a
/// path of `/setex/{key}/{ttl}/{value}` — we POST instead with a JSON
/// command array so the value passes safely through path-encoding (a
/// summary value with `/` in agent_id would otherwise break URL parsing).
pub async fn cache_summary(summary: &AgentSummary) -> Result<(), serde_json::Error> {
    let Some(cfg) = UpstashConfig::from_env() else {
        tracing::debug!("UPSTASH_REDIS_REST_URL/TOKEN unset; skipping summary cache");
        return Ok(());
    };

    let key = cache_key(summary.chain_id, &summary.agent_id);
    let payload = json!({
        "chain_id": summary.chain_id,
        "agent_id": summary.agent_id.to_string(),
        "count": summary.count,
        "mode_decimals": summary.mode_decimals,
        "mean_value": summary.mean_value.to_string(),
    });
    let value = serde_json::to_string(&payload)?;

    let url = format!("{}/", cfg.base_url);
    let body = json!(["SETEX", key, TTL_SECONDS, value]).to_string();

    let client = match reqwest::Client::builder().timeout(HTTP_TIMEOUT).build() {
        Ok(c) => c,
        Err(e) => {
            tracing::debug!(error = %e, "reqwest client build failed; skipping cache");
            return Ok(());
        }
    };

    let result = client
        .post(&url)
        .bearer_auth(&cfg.token)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await;

    match result {
        Ok(resp) if resp.status().is_success() => {
            tracing::debug!(chain_id = summary.chain_id, "cached reputation summary");
        }
        Ok(resp) => {
            tracing::debug!(status = %resp.status(), "upstash cache returned non-2xx; ignoring");
        }
        Err(e) => {
            tracing::debug!(error = %e, "upstash cache request failed; ignoring");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use bigdecimal::BigDecimal;
    use std::str::FromStr;

    #[test]
    fn cache_key_is_canonical() {
        let aid = BigDecimal::from_str("12345").unwrap();
        assert_eq!(cache_key(8453, &aid), "rep:summary:8453:12345");
        let big = BigDecimal::from_str(
            "11579208923731619542357098500868790785326998466564056403945758400791312963993",
        )
        .unwrap();
        let k = cache_key(8453, &big);
        // No spaces, no `/`, no truncation.
        assert!(!k.contains(' '));
        assert!(!k.contains('/'));
        assert!(k.starts_with("rep:summary:8453:"));
    }

    #[test]
    fn config_from_env_requires_both() {
        // Test in isolation by setting/unsetting both vars.
        let prev_url = std::env::var("UPSTASH_REDIS_REST_URL").ok();
        let prev_tok = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok();

        std::env::remove_var("UPSTASH_REDIS_REST_URL");
        std::env::remove_var("UPSTASH_REDIS_REST_TOKEN");
        assert!(UpstashConfig::from_env().is_none());

        std::env::set_var("UPSTASH_REDIS_REST_URL", "https://x.upstash.io");
        assert!(
            UpstashConfig::from_env().is_none(),
            "url alone must not enable caching"
        );

        std::env::set_var("UPSTASH_REDIS_REST_TOKEN", "tok");
        let cfg = UpstashConfig::from_env().expect("both vars set");
        assert_eq!(cfg.base_url, "https://x.upstash.io");
        assert_eq!(cfg.token, "tok");

        // Trailing slash stripped.
        std::env::set_var("UPSTASH_REDIS_REST_URL", "https://x.upstash.io/");
        let cfg = UpstashConfig::from_env().expect("trailing-slash variant");
        assert_eq!(cfg.base_url, "https://x.upstash.io");

        // Restore.
        match prev_url {
            Some(v) => std::env::set_var("UPSTASH_REDIS_REST_URL", v),
            None => std::env::remove_var("UPSTASH_REDIS_REST_URL"),
        }
        match prev_tok {
            Some(v) => std::env::set_var("UPSTASH_REDIS_REST_TOKEN", v),
            None => std::env::remove_var("UPSTASH_REDIS_REST_TOKEN"),
        }
    }

    /// `cache_summary` with no env vars set is a quiet success — workers
    /// can call it unconditionally without a guard.
    #[tokio::test]
    async fn cache_summary_no_env_is_ok() {
        // Snapshot env, clear, run, restore.
        let prev_url = std::env::var("UPSTASH_REDIS_REST_URL").ok();
        let prev_tok = std::env::var("UPSTASH_REDIS_REST_TOKEN").ok();
        std::env::remove_var("UPSTASH_REDIS_REST_URL");
        std::env::remove_var("UPSTASH_REDIS_REST_TOKEN");

        let summary = AgentSummary {
            chain_id: 8453,
            agent_id: BigDecimal::from_str("42").unwrap(),
            count: 7,
            mode_decimals: 2,
            mean_value: BigDecimal::from_str("75").unwrap(),
        };
        cache_summary(&summary).await.expect("no env -> Ok(())");

        match prev_url {
            Some(v) => std::env::set_var("UPSTASH_REDIS_REST_URL", v),
            None => std::env::remove_var("UPSTASH_REDIS_REST_URL"),
        }
        match prev_tok {
            Some(v) => std::env::set_var("UPSTASH_REDIS_REST_TOKEN", v),
            None => std::env::remove_var("UPSTASH_REDIS_REST_TOKEN"),
        }
    }
}
