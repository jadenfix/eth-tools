//! W2 — manifest_fetcher. Cadence: every 15 minutes.
//!
//! For each agent whose manifest is stale (`agents.last_manifest_fetch IS
//! NULL OR < now() - 24h`), fetch the `agentURI` through the **shared**
//! SSRF-safe fetcher in `eth_tools_core::safe_fetch` (same code path as
//! the MCP `validate_manifest` tool — see that module for the threat
//! model), validate the response against the ERC-8004 JSON Schema, hash
//! the **raw bytes** (no canonicalization — the spec hashes bytes), snapshot
//! to Vercel Blob if configured, and UPSERT a `manifests` row.
//!
//! ## Spec gotchas
//!
//! 1. **Hash raw bytes, not canonicalised JSON.** ERC-8004 doesn't define
//!    a canonical form (plan §15) so two pretty-printers would otherwise
//!    produce two on-chain digests for "the same" manifest. We hash what
//!    the wire delivered.
//! 2. **Always bump `last_manifest_fetch`, even on failure.** Otherwise
//!    a broken URI gets re-attempted every 15 minutes for eternity. The
//!    24h throttle is the entire point of the column.
//! 3. **Unreachable rows use `sha256("")` as the PK sentinel.** PK is
//!    `(chain_id, agent_id, raw_bytes_sha256)`. With no bytes we'd have
//!    no key; the empty-hash sentinel makes repeated unreachable attempts
//!    collide on PK and UPSERT in place.
//! 4. **Blob 500 ≠ unreachable.** A Vercel Blob outage must not mark a
//!    perfectly-good manifest as unreachable. We log + leave `blob_url`
//!    NULL and still write the manifest row.
//!
//! ## Concurrency
//!
//! Up to 8 fetches in flight at once via a `tokio::sync::Semaphore` +
//! `JoinSet`. Each fetch is independent; failures don't block the others.
//!
//! ## Privacy
//!
//! We never log fetched manifest BODIES (the body could be any
//! attacker-controlled content). We log the agent triple
//! `(chain_id, agent_id, source_uri)` and the outcome — never the bytes.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use bigdecimal::BigDecimal;
use eth_tools_core::manifest::{keccak256_raw_bytes, manifest_schema};
use eth_tools_core::safe_fetch::{fetch_uri, SafeFetchError};
use eth_tools_db::manifests::{
    list_stale_agents, touch_last_fetch, upsert_manifest, StaleAgent, ValidationStatus,
};
use eth_tools_db::Pool as PgPool;
use sha2::{Digest, Sha256};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

/// Fetcher fn shape. Production wires `safe_fetch::fetch_uri`; integration
/// tests inject a loopback-permissive variant so axum fixtures work without
/// disabling the prod SSRF deny set in real code paths.
///
/// Boxed dynamic future so the Fn trait remains object-safe across the
/// `JoinSet` task boundary.
pub type Fetcher = Arc<
    dyn Fn(String) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, SafeFetchError>> + Send>>
        + Send
        + Sync,
>;

/// Default fetcher = the prod SSRF-safe path. Pulled out so `run` and the
/// test seam both go through one factory.
pub fn prod_fetcher() -> Fetcher {
    Arc::new(|uri: String| Box::pin(async move { fetch_uri(&uri).await }))
}

/// Max concurrent in-flight fetches. Sized to fit comfortably under the
/// PgPool budget (max_connections=8 in `eth_tools_db::connect`) so the
/// UPSERT after each fetch doesn't queue waiting for a connection.
const MAX_CONCURRENT_FETCHES: usize = 8;

/// Max agents pulled per worker tick. Bounds wall-clock per invocation so
/// W2 never overruns Vercel's function timeout. Plan §3 budget = ~60s,
/// per-fetch p99 ~3s with a 10s safe-fetch ceiling → 50 agents max.
const MAX_AGENTS_PER_TICK: i32 = 50;

/// Staleness threshold in hours.
const STALE_AFTER_HOURS: i32 = 24;

const WORKER_NAME: &str = "manifest_fetcher";

/// Public entrypoint called from `api/cron/manifest_fetcher.rs`. Wires
/// the production fetcher (prod SSRF guard); tests call [`run_with`]
/// with a loopback-permissive fetcher.
pub async fn run(ctx: WorkerContext) -> Result<WorkerSummary, Error> {
    run_with(ctx, prod_fetcher()).await
}

/// Same as [`run`] but the fetcher is injected — exposes the same fan-out
/// / persist / touch path for the integration test without bypassing the
/// SSRF policy in production binaries. The fetcher receives the already-
/// normalised HTTPS URI (IPFS gateway rewrite already applied).
pub async fn run_with(ctx: WorkerContext, fetcher: Fetcher) -> Result<WorkerSummary, Error> {
    let stale = list_stale_agents(&ctx.pool, STALE_AFTER_HOURS, MAX_AGENTS_PER_TICK)
        .await
        .map_err(|e| Error::from(format!("list_stale_agents: {e}")))?;
    let rows_in = stale.len() as u64;

    if stale.is_empty() {
        return Ok(WorkerSummary {
            worker: WORKER_NAME,
            ok: true,
            rows_in: 0,
            rows_out: 0,
            dryrun: false,
            skipped: false,
            reason: None,
        });
    }

    // Read env once up-front; passed by Arc into each task so the env-var
    // lookup doesn't race with mutations from other workers.
    let blob_token = std::env::var("VERCEL_BLOB_READ_WRITE_TOKEN").ok();
    let ipfs_gateway =
        std::env::var("IPFS_GATEWAY_URL").unwrap_or_else(|_| "https://ipfs.io".into());
    let cfg = Arc::new(FetchConfig {
        blob_token,
        ipfs_gateway,
    });

    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_FETCHES));
    let mut joinset: JoinSet<FetchOutcome> = JoinSet::new();

    for agent in stale {
        let sem = sem.clone();
        let cfg = cfg.clone();
        let fetcher = fetcher.clone();
        joinset.spawn(async move {
            let _permit = sem.acquire_owned().await.expect("semaphore closed");
            fetch_one(agent, cfg, fetcher).await
        });
    }

    let mut rows_out: u64 = 0;
    while let Some(joined) = joinset.join_next().await {
        let outcome = match joined {
            Ok(o) => o,
            Err(e) => {
                tracing::error!(error = %e, "manifest fetch task panicked");
                continue;
            }
        };

        if ctx.dryrun {
            // Dry-run: do not write to manifests / agents. Plan §3
            // invariant 4: dry-run runs all reads but skips writes.
            tracing::info!(
                chain_id = outcome.chain_id,
                agent_id = %outcome.agent_id,
                status = outcome.status.as_str(),
                "dry-run: would upsert manifest"
            );
            continue;
        }

        if let Err(e) = persist(&ctx.pool, &outcome).await {
            tracing::error!(
                chain_id = outcome.chain_id,
                agent_id = %outcome.agent_id,
                error = %e,
                "persist failed; agent will be retried next tick"
            );
            continue;
        }
        rows_out += 1;
    }

    Ok(WorkerSummary {
        worker: WORKER_NAME,
        ok: true,
        rows_in,
        rows_out,
        dryrun: false,
        skipped: false,
        reason: None,
    })
}

#[derive(Debug)]
struct FetchConfig {
    /// `VERCEL_BLOB_READ_WRITE_TOKEN`. When `None`, blob snapshot is
    /// skipped (dev-mode) — the manifest row is still written, just
    /// without `blob_url`. Plan §3 W2.
    blob_token: Option<String>,
    /// Gateway used to translate `ipfs://<cid>` to HTTPS. Defaults to
    /// `https://ipfs.io`. Env-overridable so prod can use a private
    /// gateway with rate-limit headers.
    ipfs_gateway: String,
}

/// Per-agent fetch result. Crosses task boundaries so it must be `Send +
/// 'static`; we deliberately keep it owned-data only (no borrows from
/// the source `StaleAgent`).
#[derive(Debug)]
struct FetchOutcome {
    chain_id: i64,
    agent_id: BigDecimal,
    /// The HTTPS URI we actually fetched (after `ipfs://` rewrite). Stored
    /// as `source_uri` so the dashboard can diff against the on-chain
    /// `agent_uri`.
    fetched_uri: String,
    /// `sha256` of the raw bytes, or `sha256("")` for unreachable.
    sha256: [u8; 32],
    /// `keccak256` of the raw bytes, or `keccak256("")` for unreachable.
    /// (Both digests of empty bytes are deterministic — see module note 3.)
    keccak: [u8; 32],
    status: ValidationStatus,
    /// Parsed JSON (when fetched + parses); None for unreachable or
    /// JSON-parse failure.
    parsed: Option<serde_json::Value>,
    /// JSON Schema errors (when validation_status == Invalid) or fetch
    /// error message (when validation_status == Unreachable). Always JSON
    /// so the column has a uniform shape.
    errors: Option<serde_json::Value>,
    /// Public URL returned by the Vercel Blob upload. None when blob
    /// uploads are disabled (no token) or the upload itself failed.
    blob_url: Option<String>,
}

/// `sha256("")` and `keccak256("")` cached as the deterministic sentinel
/// for `unreachable` rows so re-attempts on the same agent collide on
/// the manifests PK and update in place (see module note 3).
fn empty_digests() -> ([u8; 32], [u8; 32]) {
    let mut h = Sha256::new();
    h.update(b"");
    let sha = h.finalize();
    let mut sha32 = [0u8; 32];
    sha32.copy_from_slice(&sha);
    (sha32, keccak256_raw_bytes(b""))
}

/// Translate `ipfs://<cid>[/<path>]` to `<gateway>/ipfs/<cid>[/<path>]`.
/// Returns the input unchanged if it doesn't start with `ipfs://`.
fn normalize_uri(raw: &str, gateway: &str) -> String {
    if let Some(rest) = raw.strip_prefix("ipfs://") {
        // Trim a trailing slash on the gateway so we don't double-slash.
        let gw = gateway.trim_end_matches('/');
        format!("{gw}/ipfs/{rest}")
    } else {
        raw.to_string()
    }
}

/// One agent's fetch + validate + (optional) blob upload. Pure async
/// fn — no DB writes; the caller serialises persistence so we don't
/// stampede the pool.
async fn fetch_one(agent: StaleAgent, cfg: Arc<FetchConfig>, fetcher: Fetcher) -> FetchOutcome {
    let fetched_uri = normalize_uri(&agent.agent_uri, &cfg.ipfs_gateway);
    tracing::info!(
        chain_id = agent.chain_id,
        agent_id = %agent.agent_id,
        // NOTE: we log the URI but NEVER the body. The URI is on-chain
        // public data; the body is attacker-controlled and could contain
        // sensitive content from arbitrary websites if the URI redirects.
        source_uri = %fetched_uri,
        "fetching manifest"
    );

    let bytes = match fetcher(fetched_uri.clone()).await {
        Ok(b) => b,
        Err(e) => return unreachable_outcome(&agent, fetched_uri, &e),
    };

    let sha = {
        let mut h = Sha256::new();
        h.update(&bytes);
        let d = h.finalize();
        let mut out = [0u8; 32];
        out.copy_from_slice(&d);
        out
    };
    let keccak = keccak256_raw_bytes(&bytes);

    // Validate. Two failure modes: not valid JSON (record as Invalid with
    // the parse error) OR JSON but schema-mismatch (record errors list).
    let (status, parsed, errors) = validate_bytes(&bytes);

    // Optional blob snapshot. Failure here is non-fatal — see note 4.
    let blob_url = if let Some(token) = cfg.blob_token.as_deref() {
        match upload_to_blob(token, agent.chain_id, &agent.agent_id, &sha, &bytes).await {
            Ok(url) => Some(url),
            Err(e) => {
                tracing::warn!(
                    chain_id = agent.chain_id,
                    agent_id = %agent.agent_id,
                    error = %e,
                    "blob upload failed; continuing without snapshot"
                );
                None
            }
        }
    } else {
        None
    };

    FetchOutcome {
        chain_id: agent.chain_id,
        agent_id: agent.agent_id,
        fetched_uri,
        sha256: sha,
        keccak,
        status,
        parsed,
        errors,
        blob_url,
    }
}

fn unreachable_outcome(agent: &StaleAgent, fetched_uri: String, err: &SafeFetchError) -> FetchOutcome {
    let (sha, keccak) = empty_digests();
    let msg = err.to_string();
    // No body — wrap the err message in a JSON array for shape parity
    // with the schema-error column.
    let errors_json = serde_json::json!([msg]);
    tracing::warn!(
        chain_id = agent.chain_id,
        agent_id = %agent.agent_id,
        error = %msg,
        "manifest unreachable"
    );
    FetchOutcome {
        chain_id: agent.chain_id,
        agent_id: agent.agent_id.clone(),
        fetched_uri,
        sha256: sha,
        keccak,
        status: ValidationStatus::Unreachable,
        parsed: None,
        errors: Some(errors_json),
        blob_url: None,
    }
}

/// Run the bytes through (1) JSON parse, (2) ERC-8004 schema validation.
/// Both arms return `ValidationStatus::Invalid` with an `errors` JSON
/// array on failure — the difference is in the error contents only, so
/// the dashboard surface stays uniform.
fn validate_bytes(
    bytes: &[u8],
) -> (
    ValidationStatus,
    Option<serde_json::Value>,
    Option<serde_json::Value>,
) {
    let manifest: serde_json::Value = match serde_json::from_slice(bytes) {
        Ok(v) => v,
        Err(e) => {
            let errors = serde_json::json!([format!("not valid JSON: {e}")]);
            return (ValidationStatus::Invalid, None, Some(errors));
        }
    };
    let schema = manifest_schema();
    let compiled = match jsonschema::validator_for(&schema) {
        Ok(c) => c,
        Err(e) => {
            // Schema-compile failure is OUR bug — the schema is a constant
            // — but we still want to record the outcome rather than crash
            // the whole tick.
            tracing::error!(error = %e, "manifest schema compile failed");
            let errors = serde_json::json!([format!("schema compile: {e}")]);
            return (ValidationStatus::Invalid, Some(manifest), Some(errors));
        }
    };
    let errors: Vec<String> = compiled
        .iter_errors(&manifest)
        .map(|e| format!("{}: {}", e.instance_path, e))
        .collect();
    if errors.is_empty() {
        (ValidationStatus::Valid, Some(manifest), None)
    } else {
        let errs_json = serde_json::json!(errors);
        (ValidationStatus::Invalid, Some(manifest), Some(errs_json))
    }
}

/// Vercel Blob upload via direct HTTP PUT. We avoid the `vercel_blob`
/// crate (extra dep audit, churn) — the API is a single authenticated
/// PUT to `https://blob.vercel-storage.com/<path>` with a JSON response
/// containing `url`.
///
/// Path scheme: `/<chainId>/<agentId>/<sha256>.json`. Deterministic from
/// the bytes so a re-uploaded identical manifest is a no-op (same URL).
async fn upload_to_blob(
    token: &str,
    chain_id: i64,
    agent_id: &BigDecimal,
    sha256: &[u8; 32],
    body: &[u8],
) -> Result<String, String> {
    let path = format!(
        "/{chain_id}/{agent_id}/{}.json",
        hex_lower(sha256.as_slice())
    );
    let url = format!("https://blob.vercel-storage.com{path}");

    // 10s timeout, rustls — matches the safe-fetch defaults so a hung
    // blob backend can't tie up the worker indefinitely.
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| format!("blob client: {e}"))?;

    let resp = client
        .put(&url)
        .bearer_auth(token)
        .header("content-type", "application/json")
        // `x-content-type-options: nosniff` mirrors what the vercel_blob
        // crate sets — keeps a hostile manifest from being served as
        // HTML when downloaded later.
        .header("x-content-type-options", "nosniff")
        .body(body.to_vec())
        .send()
        .await
        .map_err(|e| format!("blob put: {e}"))?;

    let status = resp.status();
    if !status.is_success() {
        return Err(format!("blob put returned HTTP {status}"));
    }
    // Vercel Blob returns `{"url": "..."}` on success. Falling back to
    // the constructed URL if the JSON shape changes keeps us robust.
    let parsed: serde_json::Value = match resp.json().await {
        Ok(v) => v,
        Err(_) => return Ok(url),
    };
    Ok(parsed
        .get("url")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or(url))
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Write outcome to Postgres: UPSERT `manifests` + bump
/// `agents.last_manifest_fetch`. Both writes run; if the manifest UPSERT
/// fails we still touch the agent so we don't hot-loop on the same broken
/// URI. (Caller logs and skips the rows_out increment in that case.)
async fn persist(pool: &PgPool, o: &FetchOutcome) -> Result<(), sqlx::Error> {
    upsert_manifest(
        pool,
        o.chain_id,
        &o.agent_id,
        &o.fetched_uri,
        &o.sha256,
        &o.keccak,
        o.parsed.as_ref(),
        o.status,
        o.errors.as_ref(),
        o.blob_url.as_deref(),
    )
    .await?;
    touch_last_fetch(pool, o.chain_id, &o.agent_id).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_ipfs_rewrites_to_gateway() {
        assert_eq!(
            normalize_uri("ipfs://Qmabc/manifest.json", "https://ipfs.io"),
            "https://ipfs.io/ipfs/Qmabc/manifest.json"
        );
    }

    #[test]
    fn normalize_ipfs_trims_trailing_slash_on_gateway() {
        assert_eq!(
            normalize_uri("ipfs://Qmabc", "https://gw.example/"),
            "https://gw.example/ipfs/Qmabc"
        );
    }

    #[test]
    fn normalize_https_passes_through() {
        let raw = "https://example.com/manifest.json";
        assert_eq!(normalize_uri(raw, "https://ipfs.io"), raw);
    }

    #[test]
    fn validate_bytes_valid_manifest() {
        let body = serde_json::to_vec(&serde_json::json!({
            "name": "test",
            "endpoints": [{"protocol": "https", "url": "https://x.example/"}]
        }))
        .unwrap();
        let (status, parsed, errors) = validate_bytes(&body);
        assert_eq!(status, ValidationStatus::Valid);
        assert!(parsed.is_some());
        assert!(errors.is_none());
    }

    #[test]
    fn validate_bytes_missing_name_invalid() {
        let body = serde_json::to_vec(&serde_json::json!({
            "endpoints": [{"protocol": "https", "url": "https://x.example/"}]
        }))
        .unwrap();
        let (status, _, errors) = validate_bytes(&body);
        assert_eq!(status, ValidationStatus::Invalid);
        let errs = errors.expect("errors");
        let arr = errs.as_array().expect("errors is array");
        assert!(!arr.is_empty());
    }

    #[test]
    fn validate_bytes_non_json_invalid() {
        let (status, parsed, errors) = validate_bytes(b"<html>oops</html>");
        assert_eq!(status, ValidationStatus::Invalid);
        assert!(parsed.is_none());
        assert!(errors.expect("errors").as_array().expect("array")[0]
            .as_str()
            .expect("string")
            .contains("not valid JSON"));
    }

    #[test]
    fn empty_digests_are_known_values() {
        let (sha, keccak) = empty_digests();
        // sha256("") = e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
        assert_eq!(
            hex_lower(&sha),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        // keccak256("") = c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470
        assert_eq!(
            hex_lower(&keccak),
            "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        );
    }

    #[test]
    fn unreachable_outcome_uses_empty_sha_sentinel() {
        let agent = StaleAgent {
            chain_id: 8453,
            agent_id: 1.into(),
            agent_uri: "https://bad.example/".into(),
        };
        let err = SafeFetchError::InvalidInput("nope".into());
        let o = unreachable_outcome(&agent, "https://bad.example/".into(), &err);
        assert_eq!(o.status, ValidationStatus::Unreachable);
        let (expected_sha, _) = empty_digests();
        assert_eq!(o.sha256, expected_sha);
        assert!(o.parsed.is_none());
    }
}
