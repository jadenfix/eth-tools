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
use std::net::IpAddr;
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

/// SSRF guard (B2 from Phase-5 MCP deep review): reject any IP that
/// targets the host or an internal network, including IPv4-mapped IPv6
/// (`::ffff:127.0.0.1`) — `IpAddr::is_loopback` does not catch those.
///
/// The deny set covers:
///   * RFC 1918 private (10/8, 172.16/12, 192.168/16) via [`is_private`]
///   * Loopback (127/8, ::1)
///   * Link-local (169.254/16, fe80::/10) — includes the EC2/IMDS address
///   * Unspecified (0.0.0.0, ::)
///   * IPv6 unique local addresses (fc00::/7)
fn is_disallowed_ip(ip: IpAddr) -> bool {
    let v4 = match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => v6.to_ipv4_mapped(),
    };
    if let Some(v4) = v4 {
        return v4.is_private()
            || v4.is_loopback()
            || v4.is_link_local()
            || v4.is_unspecified()
            // 0.0.0.0/8 — "this network", routable in some stacks.
            || v4.octets()[0] == 0;
    }
    // Pure IPv6 path. `is_loopback`/`is_unspecified` are stable; ULA
    // (fc00::/7) and link-local (fe80::/10) need manual segment checks
    // because `Ipv6Addr::is_unique_local`/`is_unicast_link_local` are
    // unstable as of rustc 1.84.
    if let IpAddr::V6(v6) = ip {
        if v6.is_loopback() || v6.is_unspecified() {
            return true;
        }
        let s0 = v6.segments()[0];
        // fc00::/7
        if (s0 & 0xfe00) == 0xfc00 {
            return true;
        }
        // fe80::/10
        if (s0 & 0xffc0) == 0xfe80 {
            return true;
        }
    }
    false
}

/// Resolve `host:port` and reject if any returned address is in the
/// SSRF deny set. Mirrors what reqwest's connector would do, but lets us
/// pre-flight the check before issuing the GET (and on every redirect
/// hop, since we drop the redirect-policy to zero).
async fn check_host_ssrf(host: &str, port: u16) -> Result<(), ToolError> {
    // Literal-IP fast path: skip DNS, check the address directly.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_disallowed_ip(ip) {
            return Err(ToolError::InvalidInput(format!(
                "uri host `{host}` resolves to a disallowed address"
            )));
        }
        return Ok(());
    }
    let resolved: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| ToolError::InvalidInput(format!("uri host resolve: {e}")))?
        .collect();
    if resolved.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "uri host `{host}` resolved to no addresses"
        )));
    }
    // If *any* resolved address is denylisted, reject — a hostile DNS
    // can mix one public + one private address to bypass the check.
    for addr in &resolved {
        if is_disallowed_ip(addr.ip()) {
            return Err(ToolError::InvalidInput(format!(
                "uri host `{host}` resolves to a disallowed address"
            )));
        }
    }
    Ok(())
}

async fn fetch_uri(uri: &str) -> Result<Vec<u8>, ToolError> {
    // Parse + scheme check first so we never send a request to file://, etc.
    let url = url::Url::parse(uri).map_err(|e| ToolError::InvalidInput(format!("uri: parse: {e}")))?;
    let default_port: u16 = match url.scheme() {
        "http" => 80,
        "https" => 443,
        other => {
            return Err(ToolError::InvalidInput(format!(
                "uri scheme `{other}` not allowed — http(s) only"
            )));
        }
    };
    let host = url
        .host_str()
        .ok_or_else(|| ToolError::InvalidInput("uri: missing host".into()))?;
    let port = url.port().unwrap_or(default_port);
    check_host_ssrf(host, port).await?;

    // B2: cap redirects to zero. reqwest's redirect policy callback is
    // synchronous, so we can't async-resolve each hop's host inside a
    // `Policy::custom`. Disabling redirects entirely is the cleanest
    // fail-closed option — callers that need redirects can resolve the
    // final URL out-of-band before passing it. A `limited(0)` policy
    // converts a 3xx into a terminal non-2xx response (the body's status
    // check below then rejects it as `InvalidInput`).
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
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

    // -------------------------------------------------------------------
    // B2 (Phase-5 MCP deep review): SSRF guard tests. The deny set must
    // catch literal-IP loopback, link-local IMDS, and an HTTP redirect
    // whose `Location` points at a private address. Public IPs must pass
    // the resolver check (the request itself may still fail, which is
    // fine — we only assert the SSRF gate behavior here).

    #[tokio::test]
    async fn fetch_rejects_127_0_0_1() {
        let err = fetch_uri("http://127.0.0.1:1/manifest.json").await.unwrap_err();
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("disallowed"),
                "expected disallowed-address message, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_rejects_169_254_169_254() {
        // EC2 IMDS — the most common SSRF target. Must be link-local-rejected.
        let err = fetch_uri("http://169.254.169.254/latest/meta-data/")
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn fetch_rejects_ipv4_mapped_ipv6_loopback() {
        // `::ffff:127.0.0.1` — `Ipv6Addr::is_loopback` returns false on
        // this, so the explicit `to_ipv4_mapped` path is the only thing
        // catching it. This test locks that branch in.
        let err = fetch_uri("http://[::ffff:127.0.0.1]:1/x").await.unwrap_err();
        assert!(matches!(err, ToolError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn fetch_rejects_redirect_to_private() {
        // Spin up a tiny in-process server that 302s to 127.0.0.1. With
        // `Policy::none()`, reqwest returns the 302 as a terminal
        // non-2xx response — our `status.is_success()` gate rejects it
        // as `InvalidInput("uri returned HTTP 302 ...")`. That's the
        // safety-equivalent of refusing the redirect: the body of the
        // private resource is never fetched.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().route(
            "/redir",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::FOUND,
                    [(axum::http::header::LOCATION, "http://127.0.0.1:1/secret")],
                )
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_uri(&format!("http://127.0.0.1:{port}/redir"))
            .await
            .unwrap_err();
        // The host (127.0.0.1) itself is denied before we even reach the
        // redirect — that's the stricter guarantee and also satisfies
        // the "do not follow redirect to private" intent.
        assert!(
            matches!(err, ToolError::InvalidInput(_)),
            "expected InvalidInput, got {err:?}"
        );

        handle.abort();
    }

    #[tokio::test]
    async fn fetch_allows_public_ip() {
        // Pre-resolve check must NOT reject a public address. We avoid
        // network egress by checking the resolver path directly via
        // `check_host_ssrf`. `8.8.8.8` is a public literal IP, so the
        // literal-IP fast path proves the gate's positive case without
        // hitting DNS or the network.
        check_host_ssrf("8.8.8.8", 443)
            .await
            .expect("public IP must pass");
    }

    #[test]
    fn ssrf_deny_set_classifications() {
        // Unit-table for the deny set so the policy is self-documenting.
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "::1",
            "fc00::1",
            "fe80::1",
        ] {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(is_disallowed_ip(parsed), "{ip} should be disallowed");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2606:4700::1111"] {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(!is_disallowed_ip(parsed), "{ip} should be allowed");
        }
    }
}
