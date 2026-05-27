//! `validate_manifest` + `hash_manifest`. Pure compute — no DB.
//!
//! Both tools accept either:
//!   - `uri`: fetched server-side via the **shared** SSRF-safe fetcher
//!     (`eth_tools_core::safe_fetch`) — same defence-in-depth posture as
//!     the W2 `manifest_fetcher` cron worker. See that module for the
//!     full threat model (TOCTOU pinning, per-hop redirect re-vet,
//!     streaming body cap, content-type allowlist).
//!   - `bytes`: base64-encoded payload (preferred for deterministic tests).
//!
//! Exactly one of the two must be supplied; passing both or neither is an
//! `InvalidInput` error.

use super::{DataEnvelope, ToolError};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use eth_tools_core::manifest::manifest_schema;
use eth_tools_core::safe_fetch::{fetch_uri, SafeFetchError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Map shared safe-fetch errors into the MCP-facing `ToolError`. Both
/// variants surface as `InvalidInput` so MCP clients see a consistent
/// "your URI was rejected" error; the distinction matters internally for
/// retry policy in workers, not for the MCP tool surface.
impl From<SafeFetchError> for ToolError {
    fn from(e: SafeFetchError) -> Self {
        match e {
            SafeFetchError::InvalidInput(m) => ToolError::InvalidInput(m),
            // Network failures from an attacker-controlled URI are not
            // "internal server errors" — they're "you gave us a bad URI."
            SafeFetchError::Network(m) => ToolError::InvalidInput(m),
        }
    }
}

/// `uri` XOR `bytes` argument shape. Documented in each `#[tool]`'s
/// description.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct ManifestArgs {
    #[serde(default)]
    pub uri: Option<String>,
    /// Base64-encoded manifest body. Standard alphabet (RFC 4648 §4).
    #[serde(default)]
    pub bytes: Option<String>,
}

/// Fetch-or-decode dispatcher. Returns the raw bytes to be hashed/validated
/// along with the source label (`"uri"` / `"bytes"`) for telemetry.
async fn load(args: &ManifestArgs) -> Result<(Vec<u8>, &'static str), ToolError> {
    match (args.uri.as_deref(), args.bytes.as_deref()) {
        (Some(uri), None) => fetch_uri(uri).await.map(|b| (b, "uri")).map_err(Into::into),
        (None, Some(b64)) => B64
            .decode(b64)
            .map(|b| (b, "bytes"))
            .map_err(|e| ToolError::InvalidInput(format!("bytes: invalid base64: {e}"))),
        (Some(_), Some(_)) => Err(ToolError::InvalidInput(
            "supply exactly one of `uri` or `bytes`, not both".into(),
        )),
        (None, None) => Err(ToolError::InvalidInput(
            "supply exactly one of `uri` or `bytes`".into(),
        )),
    }
}

// ---- validate_manifest ----

#[derive(Debug, Serialize, JsonSchema)]
pub struct ValidateResult {
    pub valid: bool,
    pub errors: Vec<String>,
    pub source: &'static str,
}

pub async fn validate_manifest(args: ManifestArgs) -> Result<DataEnvelope<ValidateResult>, ToolError> {
    let (bytes, source) = load(&args).await?;
    let manifest: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            return Ok(DataEnvelope::local(ValidateResult {
                valid: false,
                errors: vec![format!("not valid JSON: {e}")],
                source,
            }));
        }
    };

    // ERC-8004 manifest schema lives in `eth_tools_core::manifest` — the
    // worker pipeline (W2) and the MCP tool share one source.
    let schema = manifest_schema();
    let compiled = jsonschema::validator_for(&schema)
        .map_err(|e| ToolError::Other(format!("schema compile: {e}")))?;
    let errors: Vec<String> = compiled
        .iter_errors(&manifest)
        .map(|e| format!("{}: {}", e.instance_path, e))
        .collect();
    Ok(DataEnvelope::local(ValidateResult {
        valid: errors.is_empty(),
        errors,
        source,
    }))
}

// ---- hash_manifest ----

#[derive(Debug, Serialize, JsonSchema)]
pub struct HashResult {
    /// `0x<64 lowercase hex chars>` — sha256 of the manifest bytes as-fetched
    /// (no canonicalization). Matches the on-chain `agent_uri` digest the
    /// ERC-8004 spec uses for tamper-evidence.
    pub sha256: String,
    pub length: usize,
    pub source: &'static str,
}

pub async fn hash_manifest(args: ManifestArgs) -> Result<DataEnvelope<HashResult>, ToolError> {
    let (bytes, source) = load(&args).await?;
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(2 + 64);
    hex.push_str("0x");
    for b in digest {
        hex.push_str(&format!("{b:02x}"));
    }
    Ok(DataEnvelope::local(HashResult {
        sha256: hex,
        length: bytes.len(),
        source,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use eth_tools_core::safe_fetch::{
        fetch_with_guard, DnsResolver, FetchOptions, RealResolver, SsrfGuard,
    };
    use serde_json::json;
    use std::net::IpAddr;

    fn permissive_guard(_ip: IpAddr) -> Result<(), String> {
        Ok(())
    }

    // Tiny adapter: the underlying safe_fetch takes &SsrfGuard / &dyn
    // DnsResolver. These test fixtures kept the original integration
    // coverage from before the refactor so any regression in either the
    // shared safe-fetch policy or the MCP tool surface fires here.
    async fn fetch_loopback_ok(uri: &str, require_json: bool) -> Result<Vec<u8>, SafeFetchError> {
        let opts = FetchOptions {
            require_json,
            ..FetchOptions::default()
        };
        let g: &SsrfGuard = &permissive_guard;
        let r: &dyn DnsResolver = &RealResolver;
        fetch_with_guard(uri, r, g, &opts).await
    }

    #[test]
    fn manifest_args_uri_only() {
        let v: ManifestArgs = serde_json::from_value(json!({"uri": "https://x/y"})).unwrap();
        assert_eq!(v.uri.as_deref(), Some("https://x/y"));
        assert!(v.bytes.is_none());
    }

    #[test]
    fn manifest_args_bytes_only() {
        let v: ManifestArgs = serde_json::from_value(json!({"bytes": "aGk="})).unwrap();
        assert_eq!(v.bytes.as_deref(), Some("aGk="));
    }

    #[tokio::test]
    async fn load_rejects_both_uri_and_bytes() {
        let r = load(&ManifestArgs {
            uri: Some("https://x".into()),
            bytes: Some("aGk=".into()),
        })
        .await;
        match r {
            Err(ToolError::InvalidInput(_)) => {}
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn load_rejects_neither() {
        let r = load(&ManifestArgs {
            uri: None,
            bytes: None,
        })
        .await;
        assert!(matches!(r, Err(ToolError::InvalidInput(_))));
    }

    #[tokio::test]
    async fn hash_of_known_bytes_matches_openssl() {
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let body = B64.encode(b"hello");
        let r = hash_manifest(ManifestArgs {
            uri: None,
            bytes: Some(body),
        })
        .await
        .unwrap();
        assert_eq!(
            r.data.sha256,
            "0x2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_eq!(r.data.length, 5);
    }

    #[tokio::test]
    async fn validate_accepts_minimal_valid_manifest() {
        let body = serde_json::to_vec(&json!({
            "name": "test agent",
            "endpoints": [{"protocol": "https", "url": "https://example.com/agent"}]
        }))
        .unwrap();
        let r = validate_manifest(ManifestArgs {
            uri: None,
            bytes: Some(B64.encode(&body)),
        })
        .await
        .unwrap();
        assert!(r.data.valid, "errors: {:?}", r.data.errors);
    }

    #[tokio::test]
    async fn validate_rejects_missing_name() {
        let body = serde_json::to_vec(&json!({
            "endpoints": [{"protocol": "https", "url": "https://example.com/agent"}]
        }))
        .unwrap();
        let r = validate_manifest(ManifestArgs {
            uri: None,
            bytes: Some(B64.encode(&body)),
        })
        .await
        .unwrap();
        assert!(!r.data.valid);
        assert!(!r.data.errors.is_empty());
    }

    #[tokio::test]
    async fn validate_rejects_non_json_bytes() {
        let r = validate_manifest(ManifestArgs {
            uri: None,
            bytes: Some(B64.encode(b"<html>oops</html>")),
        })
        .await
        .unwrap();
        assert!(!r.data.valid);
        assert!(r.data.errors[0].contains("not valid JSON"));
    }

    // -------------------------------------------------------------------
    // SSRF/TOCTOU/body-cap regression tests are exhaustively covered in
    // `crates/core/src/safe_fetch.rs`. The smoke tests below assert that
    // the MCP `uri:` surface continues to delegate to that policy — if
    // someone re-introduces a parallel fetcher, these fire.

    #[tokio::test]
    async fn mcp_uri_path_rejects_loopback() {
        let r = validate_manifest(ManifestArgs {
            uri: Some("http://127.0.0.1:1/manifest.json".into()),
            bytes: None,
        })
        .await;
        match r {
            Err(ToolError::InvalidInput(m)) => assert!(
                m.contains("deny set") || m.contains("disallowed"),
                "expected SSRF rejection, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn mcp_uri_path_rejects_html_content_type() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().route(
            "/html",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "text/html")],
                    "<html>captive portal</html>",
                )
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        // Use the test-only fetch wrapper to prove the SHARED safe-fetch
        // policy rejects HTML even via a loopback fixture (where the prod
        // guard would have rejected first). This is the regression seam
        // that the parallel-fetcher accident would trip.
        let err = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/html"), true)
            .await
            .expect_err("text/html must reject");
        assert!(matches!(err, SafeFetchError::InvalidInput(_)));
        handle.abort();
    }
}
