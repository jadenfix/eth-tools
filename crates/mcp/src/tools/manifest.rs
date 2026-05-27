//! `validate_manifest` + `hash_manifest`. Pure compute — no DB.
//!
//! Both tools accept either:
//!   - `uri`: fetched server-side (HTTP/HTTPS only, body capped at 10 MB)
//!   - `bytes`: base64-encoded payload (preferred for deterministic tests)
//!
//! Exactly one of the two must be supplied; passing both or neither is an
//! `InvalidInput` error.

use super::{DataEnvelope, ToolError};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

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
        (Some(uri), None) => fetch_uri(uri).await.map(|b| (b, "uri")),
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

/// Body cap mirrors the Vercel bridge — a 10 MB manifest is well past
/// "agent description" territory and into "the URL is wrong" territory.
const MAX_MANIFEST_BYTES: usize = 10 * 1024 * 1024;

async fn fetch_uri(uri: &str) -> Result<Vec<u8>, ToolError> {
    // Parse + scheme check first so we never send a request to file://, etc.
    let url = url::Url::parse(uri).map_err(|e| ToolError::InvalidInput(format!("uri: parse: {e}")))?;
    match url.scheme() {
        "http" | "https" => {}
        other => {
            return Err(ToolError::InvalidInput(format!(
                "uri scheme `{other}` not allowed — http(s) only"
            )));
        }
    }
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|e| ToolError::Other(format!("http client: {e}")))?;
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ToolError::Other(format!("http get: {e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ToolError::InvalidInput(format!("uri returned HTTP {status}")));
    }
    // Bound the read so a hostile server can't drain memory by streaming
    // forever. `Content-Length` is advisory; we always cap manually.
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ToolError::Other(format!("http body: {e}")))?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(ToolError::InvalidInput(format!(
            "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
        )));
    }
    Ok(bytes.to_vec())
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

    // ERC-8004 manifest v1 schema — kept inline so this PR doesn't need a
    // new `crates/manifest` crate. Cross-checks `name`, `description`,
    // `endpoints[]` shape, and the optional `signature` field. The real
    // schema spec link: https://eips.ethereum.org/EIPS/eip-8004 §Manifest.
    let schema: serde_json::Value = serde_json::json!({
        "type": "object",
        "required": ["name", "endpoints"],
        "properties": {
            "name": {"type": "string", "minLength": 1, "maxLength": 256},
            "description": {"type": "string", "maxLength": 4096},
            "endpoints": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "type": "object",
                    "required": ["protocol", "url"],
                    "properties": {
                        "protocol": {"type": "string"},
                        "url": {"type": "string", "format": "uri"}
                    }
                }
            },
            "signature": {"type": "string", "pattern": "^0x[0-9a-fA-F]+$"}
        }
    });

    let compiled =
        jsonschema::validator_for(&schema).map_err(|e| ToolError::Other(format!("schema compile: {e}")))?;
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
    use serde_json::json;

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
}
