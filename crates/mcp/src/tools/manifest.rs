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
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
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

/// Maximum number of HTTP redirects we follow. Each hop is independently
/// SSRF-vetted via [`fetch_with_guard`], so the cap exists to prevent
/// pathological loops, not as the safety boundary itself.
const MAX_REDIRECTS: usize = 3;

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
///   * IPv6 6to4 wrappers (2002::/16) — unwrap embedded IPv4 and re-check,
///     so `2002:7f00:0001::/48` (6to4 of 127.0.0.1) is denied.
///   * IPv6 Teredo (2001::/32) — denied outright. Teredo is an obscure
///     IPv4-over-IPv6 tunnel; a manifest URL using it is almost certainly
///     an attempt to bypass the IPv4 deny set.
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
        let s = v6.segments();
        // fc00::/7
        if (s[0] & 0xfe00) == 0xfc00 {
            return true;
        }
        // fe80::/10
        if (s[0] & 0xffc0) == 0xfe80 {
            return true;
        }
        // 2002::/16 — 6to4. Embedded IPv4 lives in segments [1..3]
        // (the next 32 bits). Unwrap and re-check so 6to4-of-private
        // is also denied.
        if s[0] == 0x2002 {
            let embedded = std::net::Ipv4Addr::new(
                (s[1] >> 8) as u8,
                (s[1] & 0xff) as u8,
                (s[2] >> 8) as u8,
                (s[2] & 0xff) as u8,
            );
            if is_disallowed_ip(IpAddr::V4(embedded)) {
                return true;
            }
            // Even if the embedded IPv4 looks public, 6to4 routing
            // anomalies have a history of being abused. Conservative
            // policy: deny the whole 6to4 prefix.
            return true;
        }
        // 2001::/32 — Teredo. Deny outright (see module comment).
        if s[0] == 0x2001 && s[1] == 0x0000 {
            return true;
        }
    }
    false
}

/// Pluggable SSRF policy. Production code always uses [`prod_guard`];
/// tests inject relaxed guards so they can spin up loopback servers
/// without the loopback deny tripping first. The guard takes ownership
/// of the decision — it returns `Ok(())` if the IP is allowed,
/// `Err(reason)` otherwise. Reasons are surfaced verbatim as
/// `InvalidInput`.
type SsrfGuard = dyn Fn(IpAddr) -> Result<(), String> + Send + Sync;

/// Production SSRF guard: the [`is_disallowed_ip`] deny set.
fn prod_guard(ip: IpAddr) -> Result<(), String> {
    if is_disallowed_ip(ip) {
        Err(format!("address `{ip}` is in the SSRF deny set"))
    } else {
        Ok(())
    }
}

/// Pluggable DNS resolver. Tests inject a deterministic resolver to
/// prove the TOCTOU window is closed (return public IP first, private
/// second — the fetch must use the public one because we pre-resolved).
/// Production uses [`real_resolve`] which calls `tokio::net::lookup_host`.
#[async_trait::async_trait]
trait DnsResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

struct RealResolver;

#[async_trait::async_trait]
impl DnsResolver for RealResolver {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
        tokio::net::lookup_host((host, port))
            .await
            .map(|it| it.collect())
            .map_err(|e| format!("resolve `{host}`: {e}"))
    }
}

/// Pre-vet a host through the SSRF guard, returning the vetted
/// `SocketAddr`s. These addresses MUST be the only addresses reqwest
/// connects to — otherwise a hostile authoritative DNS server could
/// return a different address on reqwest's own re-resolve (TOCTOU).
///
/// Literal-IP URIs skip DNS entirely and use the parsed address.
async fn vet_host(
    host: &str,
    port: u16,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
) -> Result<Vec<SocketAddr>, ToolError> {
    // Literal-IP fast path. `host` from `Url::host_str()` strips the
    // brackets on `[::1]`, so a direct `IpAddr::parse` works.
    if let Ok(ip) = host.parse::<IpAddr>() {
        guard(ip).map_err(|m| ToolError::InvalidInput(format!("uri host `{host}`: {m}")))?;
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let resolved = resolver
        .resolve(host, port)
        .await
        .map_err(|e| ToolError::InvalidInput(format!("uri host resolve: {e}")))?;
    if resolved.is_empty() {
        return Err(ToolError::InvalidInput(format!(
            "uri host `{host}` resolved to no addresses"
        )));
    }
    // If *any* resolved address is denylisted, reject — a hostile DNS
    // can mix one public + one private address to bypass the check.
    for addr in &resolved {
        guard(addr.ip()).map_err(|m| {
            ToolError::InvalidInput(format!("uri host `{host}` -> {}: {m}", addr.ip()))
        })?;
    }
    Ok(resolved)
}

/// A reqwest DNS resolver backed by a static `host -> [SocketAddr]` map.
/// This is the keystone of the SSRF-TOCTOU fix: reqwest is given the
/// already-vetted addresses and never performs its own resolve, so a
/// hostile authoritative DNS server cannot flip the answer between
/// our pre-flight check and reqwest's connect.
struct StaticResolver {
    map: HashMap<String, Vec<SocketAddr>>,
}

impl reqwest::dns::Resolve for StaticResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        // `BoxError` is `pub(crate)` in reqwest 0.12, so we inline the
        // alias here. Any error implementing `Error + Send + Sync` works.
        type BoxError = Box<dyn std::error::Error + Send + Sync>;
        let key = name.as_str().to_string();
        let addrs = self.map.get(&key).cloned().unwrap_or_default();
        Box::pin(async move {
            let iter: Box<dyn Iterator<Item = SocketAddr> + Send> =
                Box::new(addrs.into_iter());
            Ok::<_, BoxError>(iter)
        })
    }
}

/// Vet (host, port) and build a one-shot `reqwest::Client` whose only
/// resolvable addresses are the vetted set. Returns the client and the
/// vetted address list for diagnostics.
async fn build_pinned_client(
    host: &str,
    port: u16,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
) -> Result<reqwest::Client, ToolError> {
    let addrs = vet_host(host, port, resolver, guard).await?;
    let mut map: HashMap<String, Vec<SocketAddr>> = HashMap::new();
    map.insert(host.to_string(), addrs);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        // We perform our OWN redirect loop in `fetch_with_guard` so we
        // can re-vet every hop's host through the SSRF guard. Reqwest's
        // redirect callback is sync and cannot do async DNS, which is
        // why a hand-rolled loop is the only safe option.
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(StaticResolver { map }))
        .build()
        .map_err(|e| ToolError::Other(format!("http client: {e}")))?;
    Ok(client)
}

/// Extract scheme + host + port from a parsed URL, rejecting non-HTTP(S)
/// schemes and missing hosts.
fn split_url(url: &url::Url) -> Result<(String, u16), ToolError> {
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
        .ok_or_else(|| ToolError::InvalidInput("uri: missing host".into()))?
        .to_string();
    let port = url.port().unwrap_or(default_port);
    Ok((host, port))
}

/// Production entrypoint: vet + fetch using real DNS and the prod guard.
async fn fetch_uri(uri: &str) -> Result<Vec<u8>, ToolError> {
    fetch_with_guard(uri, &RealResolver, &prod_guard, /*require_json=*/ true).await
}

/// Core fetch loop. Steps:
///   1. parse URL, split host/port, vet via [`build_pinned_client`].
///   2. issue GET. If 2xx, stream-body with a hard cap (Issue 2).
///   3. If 3xx, read `Location`, resolve relative against current URL,
///      ensure same scheme rules, increment hop counter, loop.
///   4. Anything else: `InvalidInput`.
///
/// `require_json` toggles the content-type check (Issue 5). Only `true`
/// for `uri:`-sourced fetches; the `bytes:` path never reaches here.
async fn fetch_with_guard(
    uri: &str,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
    require_json: bool,
) -> Result<Vec<u8>, ToolError> {
    let mut current = url::Url::parse(uri)
        .map_err(|e| ToolError::InvalidInput(format!("uri: parse: {e}")))?;
    let mut hops = 0usize;

    loop {
        let (host, port) = split_url(&current)?;
        let client = build_pinned_client(&host, port, resolver, guard).await?;
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| ToolError::Other(format!("http get: {e}")))?;

        let status = resp.status();
        if status.is_redirection() {
            // Manual redirect with per-hop SSRF vet (Issue 3).
            if hops >= MAX_REDIRECTS {
                return Err(ToolError::InvalidInput(format!(
                    "too many redirects (max {MAX_REDIRECTS})"
                )));
            }
            let loc = resp
                .headers()
                .get(http::header::LOCATION)
                .ok_or_else(|| ToolError::InvalidInput(format!("HTTP {status} without Location")))?
                .to_str()
                .map_err(|e| ToolError::InvalidInput(format!("Location header not ASCII: {e}")))?
                .to_string();
            // `Url::join` handles both absolute and relative `Location`.
            let next = current
                .join(&loc)
                .map_err(|e| ToolError::InvalidInput(format!("redirect target parse: {e}")))?;
            current = next;
            hops += 1;
            continue;
        }
        if !status.is_success() {
            return Err(ToolError::InvalidInput(format!("uri returned HTTP {status}")));
        }

        // Issue 5: content-type sanity. `application/json` with any
        // parameters (`; charset=utf-8`, etc) is fine.
        if require_json {
            let ct = resp
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let ct_lower = ct.to_ascii_lowercase();
            let ct_main = ct_lower.split(';').next().unwrap_or("").trim();
            if ct_main != "application/json" {
                return Err(ToolError::InvalidInput(format!(
                    "expected application/json content-type, got `{ct}`"
                )));
            }
        }

        // Issue 2: short-circuit obvious oversize via Content-Length, then
        // stream-with-cap. We never call `.bytes()` on the full body.
        if let Some(cl) = resp.content_length() {
            if cl as usize > MAX_MANIFEST_BYTES {
                return Err(ToolError::InvalidInput(format!(
                    "manifest content-length {cl} exceeds {MAX_MANIFEST_BYTES} bytes"
                )));
            }
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut resp = resp;
        loop {
            let chunk = resp
                .chunk()
                .await
                .map_err(|e| ToolError::Other(format!("http body: {e}")))?;
            let Some(chunk) = chunk else { break };
            // Cap check — abort the moment the running total exceeds
            // the limit. Buffer size is bounded by MAX + one chunk
            // (the chunk we abort on is the one that crossed it).
            if buf.len().saturating_add(chunk.len()) > MAX_MANIFEST_BYTES {
                return Err(ToolError::InvalidInput(format!(
                    "manifest exceeds {MAX_MANIFEST_BYTES} bytes"
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(buf);
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

    // Convenience wrapper that bypasses the SSRF guard for loopback
    // testing — used by every test that spins up an in-process axum
    // server on 127.0.0.1. Real deployments never use this path.
    fn permissive_guard(_ip: IpAddr) -> Result<(), String> {
        Ok(())
    }

    async fn fetch_loopback_ok(uri: &str, require_json: bool) -> Result<Vec<u8>, ToolError> {
        fetch_with_guard(uri, &RealResolver, &permissive_guard, require_json).await
    }

    // Reject-private guard that lies: it allows ALL loopback so the
    // test server is reachable, but still rejects link-local /
    // RFC-1918 to simulate "real production guard + test server."
    // Only used by the redirect-to-private test where we want the
    // initial 127.0.0.1 hop allowed but a `Location: http://10.0.0.1`
    // hop blocked.
    fn loopback_ok_rfc1918_blocked(ip: IpAddr) -> Result<(), String> {
        if let IpAddr::V4(v4) = ip {
            if v4.is_private() {
                return Err(format!("`{ip}` is RFC1918"));
            }
            // Allow loopback + link-local + everything else.
        }
        Ok(())
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
                m.contains("deny set") || m.contains("disallowed"),
                "expected deny-set message, got: {m}"
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
    async fn vet_host_allows_public_literal_ip() {
        // Positive path for the production guard: a public literal IP
        // must vet through cleanly. The fast path skips DNS, so this
        // is purely an assertion on `prod_guard`'s allow-list.
        vet_host("8.8.8.8", 443, &RealResolver, &prod_guard)
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

    // -------------------------------------------------------------------
    // Issue 4: IPv6 wrapper bypass coverage.

    #[test]
    fn ssrf_denies_6to4_of_loopback() {
        // 2002:7f00:0001::/48 — 6to4-encapsulated 127.0.0.1. The bug we
        // are guarding against is that `is_loopback`/`to_ipv4_mapped`
        // both return false on this, but the IPv4 embedded in the
        // segments [1..3] is 127.0.0.1.
        let ip: IpAddr = "2002:7f00:0001::1".parse().unwrap();
        assert!(is_disallowed_ip(ip), "6to4 of 127.0.0.1 must be denied");
    }

    #[test]
    fn ssrf_denies_6to4_of_public() {
        // Even 6to4 of a public IPv4 is denied (conservative). 8.8.8.8
        // -> 2002:0808:0808::/48
        let ip: IpAddr = "2002:0808:0808::1".parse().unwrap();
        assert!(is_disallowed_ip(ip), "6to4 prefix must be denied wholesale");
    }

    #[test]
    fn ssrf_denies_teredo() {
        // 2001:0::/32 — Teredo. Deny outright.
        let ip: IpAddr = "2001:0:0:0::1".parse().unwrap();
        assert!(is_disallowed_ip(ip), "Teredo prefix must be denied");
    }

    // -------------------------------------------------------------------
    // Issue 1: SSRF TOCTOU. The fix replaces reqwest's DNS resolver with
    // a `StaticResolver` pre-populated with the vetted addresses. The
    // contract this test enforces: even if a hostile DNS would return a
    // private IP on a second call, reqwest never re-resolves, so the
    // fetch uses the pre-vetted public address.

    /// Mock resolver that returns ONE address on first call and a
    /// DIFFERENT address on every subsequent call. If our pre-vet logic
    /// were to leak the host through to reqwest's resolver, the second
    /// call would supply a private IP and the test would observe a
    /// loopback connection. Instead, we observe the pre-vetted IP.
    struct ToctouResolver {
        first: SocketAddr,
        second: SocketAddr,
        call_count: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DnsResolver for ToctouResolver {
        async fn resolve(&self, _host: &str, _port: u16) -> Result<Vec<SocketAddr>, String> {
            let n = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                Ok(vec![self.first])
            } else {
                Ok(vec![self.second])
            }
        }
    }

    #[tokio::test]
    async fn toctou_pre_resolved_ip_pinned_into_client() {
        // Spin up a real server on 127.0.0.1:<port> that returns a
        // valid JSON body. The "public" IP we hand the resolver is the
        // server's address. The "private" IP we'd flip to is a
        // black-hole address (10.255.255.1) — if reqwest were to
        // re-resolve, it would try to connect to that address and
        // either hang or fail. We assert the fetch SUCCEEDS, proving
        // reqwest used the pinned address from the first resolve.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = listener.local_addr().unwrap();
        let app = axum::Router::new().route(
            "/m",
            axum::routing::get(|| async {
                (
                    [(axum::http::header::CONTENT_TYPE, "application/json")],
                    "{\"ok\":true}",
                )
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let toctou = ToctouResolver {
            first: server_addr,
            // Black-hole the second resolve. If this address gets used,
            // the test would fail (timeout or connection error).
            second: SocketAddr::new("10.255.255.1".parse().unwrap(), 1),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        };
        // Use a permissive guard that doesn't reject loopback, so the
        // pre-vet step passes. The contract under test is that reqwest
        // never re-resolves and therefore never sees the second address.
        let bytes = fetch_with_guard(
            &format!("http://fake.test:{}/m", server_addr.port()),
            &toctou,
            &permissive_guard,
            /*require_json=*/ true,
        )
        .await
        .expect("pinned resolver must succeed");
        assert_eq!(bytes, b"{\"ok\":true}");
        // The resolver must have been called exactly once — reqwest's
        // own connect MUST NOT have re-resolved (otherwise the second
        // resolve would have flipped to the black hole and the
        // connection would fail). Note that the mock resolver itself is
        // ours; reqwest's StaticResolver is a separate object that
        // looks up in the pre-populated map only.
        assert_eq!(
            toctou
                .call_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "pre-vet must call the resolver exactly once"
        );
        handle.abort();
    }

    // -------------------------------------------------------------------
    // Issue 2: streaming body cap. A hostile server can announce a huge
    // Content-Length (instant reject) or omit it and stream forever
    // (stream-time cap kicks in).

    #[tokio::test]
    async fn fetch_rejects_oversize_content_length() {
        // Server announces a 512 MiB Content-Length but writes only a
        // few bytes — our check must short-circuit at the HEADER, not
        // wait for buffering. Hand-rolled because axum/hyper refuses to
        // emit a Content-Length that disagrees with the body length.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 4096];
            let _ = sock.read(&mut tmp).await;
            // Announce 512 MiB, send nothing — the client must reject
            // on the header alone, never pulling body bytes.
            let resp = "HTTP/1.1 200 OK\r\n\
                        Content-Type: application/json\r\n\
                        Content-Length: 536870912\r\n\
                        Connection: close\r\n\r\n";
            let _ = sock.write_all(resp.as_bytes()).await;
            // Hold the socket open briefly so the client sees headers.
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(2),
                tokio::io::AsyncWriteExt::shutdown(&mut sock),
            )
            .await;
        });

        let err = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/big"), true)
            .await
            .expect_err("oversize content-length must reject");
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("content-length") || m.contains("exceeds"),
                "expected oversize message, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_rejects_oversize_streamed_body() {
        // Server omits Content-Length and streams more than MAX bytes
        // using chunked transfer-encoding. Cap must fire mid-stream —
        // buffered bytes ≤ MAX + one chunk. We hand-roll HTTP/1.1 on a
        // raw TCP socket so we don't pull `futures-util` / `http-body-util`
        // into dev-deps just for one fixture.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            // Drain the request — we don't care about its content.
            let mut tmp = [0u8; 4096];
            let _ = sock.read(&mut tmp).await;
            // Write the headers, then stream chunked junk past MAX.
            let headers = "HTTP/1.1 200 OK\r\n\
                           Content-Type: application/json\r\n\
                           Transfer-Encoding: chunked\r\n\
                           Connection: close\r\n\r\n";
            if sock.write_all(headers.as_bytes()).await.is_err() {
                return;
            }
            // 64 KiB chunks. After ~MAX/64KiB chunks the reader trips
            // the cap and drops the connection — we tolerate write
            // errors after that.
            let chunk = vec![b'x'; 64 * 1024];
            let chunk_hex = format!("{:x}\r\n", chunk.len());
            for _ in 0..((MAX_MANIFEST_BYTES / chunk.len()) + 8) {
                if sock.write_all(chunk_hex.as_bytes()).await.is_err() {
                    return;
                }
                if sock.write_all(&chunk).await.is_err() {
                    return;
                }
                if sock.write_all(b"\r\n").await.is_err() {
                    return;
                }
            }
            let _ = sock.write_all(b"0\r\n\r\n").await;
        });

        let err = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/stream"), true)
            .await
            .expect_err("oversize streamed body must reject");
        assert!(matches!(err, ToolError::InvalidInput(_)));
        handle.abort();
    }

    // -------------------------------------------------------------------
    // Issue 3: bounded redirects with per-hop SSRF re-vet.

    #[tokio::test]
    async fn fetch_follows_redirect_to_same_host() {
        // Server redirects /a -> /b on the same host. Both hops are
        // 127.0.0.1, so with the permissive loopback guard this must
        // succeed — proving the redirect loop works end-to-end.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new()
            .route(
                "/a",
                axum::routing::get(move || async move {
                    (
                        axum::http::StatusCode::FOUND,
                        [(axum::http::header::LOCATION, "/b")],
                    )
                }),
            )
            .route(
                "/b",
                axum::routing::get(|| async {
                    (
                        [(axum::http::header::CONTENT_TYPE, "application/json")],
                        "{\"hop\":2}",
                    )
                }),
            );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bytes = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/a"), true)
            .await
            .expect("redirect to public-ish host must succeed");
        assert_eq!(bytes, b"{\"hop\":2}");
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_rejects_redirect_to_private() {
        // Server (allowed via loopback exception) redirects to
        // 10.0.0.1 which is RFC1918. The custom guard rejects it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().route(
            "/redir",
            axum::routing::get(|| async {
                (
                    axum::http::StatusCode::FOUND,
                    [(axum::http::header::LOCATION, "http://10.0.0.1:1/secret")],
                )
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let err = fetch_with_guard(
            &format!("http://127.0.0.1:{port}/redir"),
            &RealResolver,
            &loopback_ok_rfc1918_blocked,
            /*require_json=*/ true,
        )
        .await
        .expect_err("redirect to 10/8 must reject");
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("RFC1918") || m.contains("deny") || m.contains("disallowed"),
                "expected SSRF rejection on hop, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    // -------------------------------------------------------------------
    // Issue 5: content-type check before hashing.

    #[tokio::test]
    async fn fetch_rejects_text_html_content_type() {
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

        let err = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/html"), true)
            .await
            .expect_err("text/html must reject for uri-fetch");
        match err {
            ToolError::InvalidInput(m) => assert!(
                m.contains("application/json"),
                "expected content-type rejection, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_accepts_json_with_charset_param() {
        // `application/json; charset=utf-8` MUST be accepted (RFC 6838 §4.3).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let app = axum::Router::new().route(
            "/j",
            axum::routing::get(|| async {
                (
                    [(
                        axum::http::header::CONTENT_TYPE,
                        "application/json; charset=utf-8",
                    )],
                    "{\"k\":1}",
                )
            }),
        );
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let bytes = fetch_loopback_ok(&format!("http://127.0.0.1:{port}/j"), true)
            .await
            .expect("json+charset must accept");
        assert_eq!(bytes, b"{\"k\":1}");
        handle.abort();
    }
}
