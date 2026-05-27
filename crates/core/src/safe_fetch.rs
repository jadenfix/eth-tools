//! SSRF-safe HTTP(S) fetcher shared by `crates/mcp` (the `validate_manifest`
//! / `hash_manifest` MCP tools) and `crates/workers` (the W2
//! `manifest_fetcher` cron). Both pull attacker-controlled URLs (`agentURI`
//! is registered on-chain by anyone) and need an identical defence-in-depth
//! posture; lifting the implementation here keeps the policy single-sourced.
//!
//! ## Threat model
//!
//! An attacker-controlled URL can:
//!   1. Point at an internal address (RFC1918, loopback, link-local IMDS,
//!      ULA, IPv6 wrappers like 6to4/Teredo) → **SSRF deny set**.
//!   2. Use a hostile authoritative DNS that returns a public IP on the
//!      pre-flight resolve and a private IP on reqwest's own re-resolve
//!      → **TOCTOU pinning** via [`StaticResolver`].
//!   3. Redirect to (1) or (2) on a follow-up hop → **manual redirect
//!      loop with per-hop re-vet** (cap = 3 hops).
//!   4. Stream gigabytes to OOM us → **streaming cap** (10 MB) + a
//!      `Content-Length` short-circuit.
//!   5. Serve `text/html` (captive portal, login wall) instead of JSON
//!      → **Content-Type allowlist** (toggleable via the
//!      [`FetchOptions::require_json`] flag — workers always require it).
//!   6. Stall the socket forever → **per-hop 10s timeout**.
//!
//! ## Public surface
//!
//! * [`SafeFetchError`] — domain error; callers map to their own error type.
//! * [`FetchOptions`] — knobs (max body, hop cap, JSON enforcement).
//! * [`fetch_uri`] — production entrypoint with the prod SSRF guard +
//!   real DNS.
//! * [`fetch_with_guard`] / [`DnsResolver`] / [`SsrfGuard`] — exposed for
//!   tests that need to inject a permissive guard so loopback fixtures
//!   work without bypassing the prod deny set.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

/// Errors returned by the safe fetcher. Two flavours so callers can
/// distinguish "the URI itself was rejected by the SSRF policy" from
/// "the network call failed at the transport layer."
#[derive(Debug, thiserror::Error)]
pub enum SafeFetchError {
    /// The input URI (scheme, host, redirect target, body size, etc.) was
    /// rejected by the policy. **No network request was made for the
    /// rejected hop**, or the network request returned data we refused
    /// to deliver to the caller (HTTP error, content-type mismatch,
    /// over-cap body).
    #[error("invalid input: {0}")]
    InvalidInput(String),
    /// Transport-level failure (connect, TLS, body decode). Distinct from
    /// `InvalidInput` so callers can decide whether to retry.
    #[error("network: {0}")]
    Network(String),
}

/// Knobs for [`fetch_with_guard`]. Defaults match the MCP tool surface
/// (10 MB cap, 3 redirect hops, JSON-required) — workers use the same
/// defaults to keep the policy unified.
#[derive(Debug, Clone)]
pub struct FetchOptions {
    /// Hard cap on response body bytes. Streamed: the moment the running
    /// total would exceed this, we abort and return [`SafeFetchError::InvalidInput`].
    pub max_body_bytes: usize,
    /// Maximum redirect hops we follow before giving up.
    pub max_redirects: usize,
    /// If `true`, the response's `Content-Type` MUST be `application/json`
    /// (optionally with a parameter like `; charset=utf-8`). Used by both
    /// manifest fetchers — captive portals love serving `text/html`.
    pub require_json: bool,
    /// Per-hop timeout (connect + read). Applied to each redirect hop
    /// independently so a chain can't burn `n * timeout` wall-clock.
    pub per_hop_timeout: Duration,
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            max_body_bytes: 10 * 1024 * 1024,
            max_redirects: 3,
            require_json: true,
            per_hop_timeout: Duration::from_secs(10),
        }
    }
}

/// Production SSRF deny set. Rejects:
///   * RFC 1918 private (10/8, 172.16/12, 192.168/16) via [`Ipv4Addr::is_private`].
///   * Loopback (127/8, ::1).
///   * Link-local (169.254/16, fe80::/10) — includes EC2 IMDS.
///   * Unspecified (0.0.0.0, ::).
///   * 0.0.0.0/8 ("this network", routable in some stacks).
///   * IPv4-mapped IPv6 (`::ffff:0:0/96`) — unwrap and re-check.
///   * IPv6 ULA (fc00::/7).
///   * IPv6 6to4 (2002::/16) — denied wholesale even if the embedded
///     IPv4 is public (6to4 routing anomalies have a history of abuse).
///   * IPv6 Teredo (2001:0::/32) — denied outright.
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
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
        // 2002::/16 — 6to4. Embedded IPv4 lives in segments [1..3].
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
            // Conservative: deny the whole 6to4 prefix.
            return true;
        }
        // 2001:0::/32 — Teredo.
        if s[0] == 0x2001 && s[1] == 0x0000 {
            return true;
        }
    }
    false
}

/// Pluggable SSRF policy. Production uses [`prod_guard`]; tests inject
/// relaxed guards so loopback fixtures work. The guard takes ownership
/// of the decision — `Ok(())` allows, `Err(reason)` rejects (surfaced
/// verbatim as `InvalidInput`).
pub type SsrfGuard = dyn Fn(IpAddr) -> Result<(), String> + Send + Sync;

/// Production guard: rejects anything in [`is_disallowed_ip`].
pub fn prod_guard(ip: IpAddr) -> Result<(), String> {
    if is_disallowed_ip(ip) {
        Err(format!("address `{ip}` is in the SSRF deny set"))
    } else {
        Ok(())
    }
}

/// Pluggable DNS resolver. Tests use a deterministic mock to prove the
/// TOCTOU window is closed; production uses [`RealResolver`].
#[async_trait::async_trait]
pub trait DnsResolver: Send + Sync {
    async fn resolve(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, String>;
}

/// Real DNS via `tokio::net::lookup_host`.
pub struct RealResolver;

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
/// `SocketAddr`s. These addresses MUST be the only ones reqwest connects
/// to — otherwise a hostile authoritative DNS server could flip the
/// answer between this check and reqwest's own resolve.
async fn vet_host(
    host: &str,
    port: u16,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
) -> Result<Vec<SocketAddr>, SafeFetchError> {
    // Literal-IP fast path.
    if let Ok(ip) = host.parse::<IpAddr>() {
        guard(ip).map_err(|m| SafeFetchError::InvalidInput(format!("uri host `{host}`: {m}")))?;
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    let resolved = resolver
        .resolve(host, port)
        .await
        .map_err(|e| SafeFetchError::InvalidInput(format!("uri host resolve: {e}")))?;
    if resolved.is_empty() {
        return Err(SafeFetchError::InvalidInput(format!(
            "uri host `{host}` resolved to no addresses"
        )));
    }
    // If *any* resolved address is denylisted, reject — a hostile DNS
    // can mix one public + one private address to bypass the check.
    for addr in &resolved {
        guard(addr.ip()).map_err(|m| {
            SafeFetchError::InvalidInput(format!("uri host `{host}` -> {}: {m}", addr.ip()))
        })?;
    }
    Ok(resolved)
}

/// reqwest DNS resolver backed by a static `host -> [SocketAddr]` map.
/// Keystone of the TOCTOU fix: reqwest is given the pre-vetted addresses
/// and never performs its own resolve.
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
            let iter: Box<dyn Iterator<Item = SocketAddr> + Send> = Box::new(addrs.into_iter());
            Ok::<_, BoxError>(iter)
        })
    }
}

/// Vet (host, port) and build a one-shot `reqwest::Client` whose only
/// resolvable addresses are the vetted set.
async fn build_pinned_client(
    host: &str,
    port: u16,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
    timeout: Duration,
) -> Result<reqwest::Client, SafeFetchError> {
    let addrs = vet_host(host, port, resolver, guard).await?;
    let mut map: HashMap<String, Vec<SocketAddr>> = HashMap::new();
    map.insert(host.to_string(), addrs);
    let client = reqwest::Client::builder()
        .timeout(timeout)
        // Manual redirect loop in `fetch_with_guard` re-vets every hop's
        // host. Reqwest's redirect callback is sync and can't do async
        // DNS, which is why we hand-roll the loop.
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(StaticResolver { map }))
        .build()
        .map_err(|e| SafeFetchError::Network(format!("http client: {e}")))?;
    Ok(client)
}

/// Extract host + port from a parsed URL, rejecting non-HTTP(S) schemes.
fn split_url(url: &url::Url) -> Result<(String, u16), SafeFetchError> {
    let default_port: u16 = match url.scheme() {
        "http" => 80,
        "https" => 443,
        other => {
            return Err(SafeFetchError::InvalidInput(format!(
                "uri scheme `{other}` not allowed — http(s) only"
            )));
        }
    };
    let host = url
        .host_str()
        .ok_or_else(|| SafeFetchError::InvalidInput("uri: missing host".into()))?
        .to_string();
    let port = url.port().unwrap_or(default_port);
    Ok((host, port))
}

/// Production entrypoint: real DNS + prod SSRF guard + default
/// [`FetchOptions`].
pub async fn fetch_uri(uri: &str) -> Result<Vec<u8>, SafeFetchError> {
    fetch_with_guard(uri, &RealResolver, &prod_guard, &FetchOptions::default()).await
}

/// Core fetch loop. Vets the host, opens a pinned client, follows
/// redirects manually with per-hop re-vet, streams the body with a
/// running cap. Test seam: callers inject a `DnsResolver` and `SsrfGuard`
/// so loopback fixtures can pass through without bypassing the prod
/// deny set in real code paths.
pub async fn fetch_with_guard(
    uri: &str,
    resolver: &dyn DnsResolver,
    guard: &SsrfGuard,
    opts: &FetchOptions,
) -> Result<Vec<u8>, SafeFetchError> {
    let mut current = url::Url::parse(uri)
        .map_err(|e| SafeFetchError::InvalidInput(format!("uri: parse: {e}")))?;
    let mut hops = 0usize;

    loop {
        let (host, port) = split_url(&current)?;
        let client = build_pinned_client(&host, port, resolver, guard, opts.per_hop_timeout).await?;
        let resp = client
            .get(current.clone())
            .send()
            .await
            .map_err(|e| SafeFetchError::Network(format!("http get: {e}")))?;

        let status = resp.status();
        if status.is_redirection() {
            if hops >= opts.max_redirects {
                return Err(SafeFetchError::InvalidInput(format!(
                    "too many redirects (max {})",
                    opts.max_redirects
                )));
            }
            let loc = resp
                .headers()
                .get(http::header::LOCATION)
                .ok_or_else(|| {
                    SafeFetchError::InvalidInput(format!("HTTP {status} without Location"))
                })?
                .to_str()
                .map_err(|e| {
                    SafeFetchError::InvalidInput(format!("Location header not ASCII: {e}"))
                })?
                .to_string();
            let next = current
                .join(&loc)
                .map_err(|e| SafeFetchError::InvalidInput(format!("redirect target parse: {e}")))?;
            current = next;
            hops += 1;
            continue;
        }
        if !status.is_success() {
            return Err(SafeFetchError::InvalidInput(format!(
                "uri returned HTTP {status}"
            )));
        }

        if opts.require_json {
            let ct = resp
                .headers()
                .get(http::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .unwrap_or("");
            let ct_lower = ct.to_ascii_lowercase();
            let ct_main = ct_lower.split(';').next().unwrap_or("").trim();
            if ct_main != "application/json" {
                return Err(SafeFetchError::InvalidInput(format!(
                    "expected application/json content-type, got `{ct}`"
                )));
            }
        }

        // Short-circuit obvious oversize via Content-Length, then stream
        // with a running cap. Never call `.bytes()` on the full body.
        if let Some(cl) = resp.content_length() {
            if cl as usize > opts.max_body_bytes {
                return Err(SafeFetchError::InvalidInput(format!(
                    "manifest content-length {cl} exceeds {} bytes",
                    opts.max_body_bytes
                )));
            }
        }

        let mut buf: Vec<u8> = Vec::new();
        let mut resp = resp;
        loop {
            let chunk = resp
                .chunk()
                .await
                .map_err(|e| SafeFetchError::Network(format!("http body: {e}")))?;
            let Some(chunk) = chunk else { break };
            if buf.len().saturating_add(chunk.len()) > opts.max_body_bytes {
                return Err(SafeFetchError::InvalidInput(format!(
                    "manifest exceeds {} bytes",
                    opts.max_body_bytes
                )));
            }
            buf.extend_from_slice(&chunk);
        }
        return Ok(buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permissive_guard(_ip: IpAddr) -> Result<(), String> {
        Ok(())
    }

    fn loopback_ok_rfc1918_blocked(ip: IpAddr) -> Result<(), String> {
        if let IpAddr::V4(v4) = ip {
            if v4.is_private() {
                return Err(format!("`{ip}` is RFC1918"));
            }
        }
        Ok(())
    }

    async fn fetch_loopback_ok(uri: &str, require_json: bool) -> Result<Vec<u8>, SafeFetchError> {
        let opts = FetchOptions {
            require_json,
            ..FetchOptions::default()
        };
        fetch_with_guard(uri, &RealResolver, &permissive_guard, &opts).await
    }

    #[test]
    fn ssrf_deny_set_classifications() {
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

    #[test]
    fn ssrf_denies_6to4_of_loopback() {
        let ip: IpAddr = "2002:7f00:0001::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[test]
    fn ssrf_denies_6to4_of_public() {
        let ip: IpAddr = "2002:0808:0808::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[test]
    fn ssrf_denies_teredo() {
        let ip: IpAddr = "2001:0:0:0::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[tokio::test]
    async fn fetch_rejects_127_0_0_1() {
        let err = fetch_uri("http://127.0.0.1:1/manifest.json").await.unwrap_err();
        match err {
            SafeFetchError::InvalidInput(m) => assert!(
                m.contains("deny set") || m.contains("disallowed"),
                "expected deny-set message, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn fetch_rejects_169_254_169_254() {
        let err = fetch_uri("http://169.254.169.254/latest/meta-data/")
            .await
            .unwrap_err();
        assert!(matches!(err, SafeFetchError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn fetch_rejects_ipv4_mapped_ipv6_loopback() {
        let err = fetch_uri("http://[::ffff:127.0.0.1]:1/x").await.unwrap_err();
        assert!(matches!(err, SafeFetchError::InvalidInput(_)));
    }

    #[tokio::test]
    async fn vet_host_allows_public_literal_ip() {
        vet_host("8.8.8.8", 443, &RealResolver, &prod_guard)
            .await
            .expect("public IP must pass");
    }

    // ---- TOCTOU: pre-resolved IP pinned into the reqwest client. ----

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
            second: SocketAddr::new("10.255.255.1".parse().unwrap(), 1),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        };
        let bytes = fetch_with_guard(
            &format!("http://fake.test:{}/m", server_addr.port()),
            &toctou,
            &permissive_guard,
            &FetchOptions::default(),
        )
        .await
        .expect("pinned resolver must succeed");
        assert_eq!(bytes, b"{\"ok\":true}");
        assert_eq!(
            toctou
                .call_count
                .load(std::sync::atomic::Ordering::SeqCst),
            1,
            "pre-vet must call the resolver exactly once"
        );
        handle.abort();
    }

    // ---- Body cap: Content-Length short-circuit + stream-time cap. ----

    #[tokio::test]
    async fn fetch_rejects_oversize_content_length() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 4096];
            let _ = sock.read(&mut tmp).await;
            let resp = "HTTP/1.1 200 OK\r\n\
                        Content-Type: application/json\r\n\
                        Content-Length: 536870912\r\n\
                        Connection: close\r\n\r\n";
            let _ = sock.write_all(resp.as_bytes()).await;
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
            SafeFetchError::InvalidInput(m) => assert!(
                m.contains("content-length") || m.contains("exceeds"),
                "expected oversize message, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_rejects_oversize_streamed_body() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut tmp = [0u8; 4096];
            let _ = sock.read(&mut tmp).await;
            let headers = "HTTP/1.1 200 OK\r\n\
                           Content-Type: application/json\r\n\
                           Transfer-Encoding: chunked\r\n\
                           Connection: close\r\n\r\n";
            if sock.write_all(headers.as_bytes()).await.is_err() {
                return;
            }
            let chunk = vec![b'x'; 64 * 1024];
            let chunk_hex = format!("{:x}\r\n", chunk.len());
            let max = FetchOptions::default().max_body_bytes;
            for _ in 0..((max / chunk.len()) + 8) {
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
        assert!(matches!(err, SafeFetchError::InvalidInput(_)));
        handle.abort();
    }

    // ---- Redirects: per-hop SSRF re-vet + hop cap. ----

    #[tokio::test]
    async fn fetch_follows_redirect_to_same_host() {
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
            &FetchOptions::default(),
        )
        .await
        .expect_err("redirect to 10/8 must reject");
        match err {
            SafeFetchError::InvalidInput(m) => assert!(
                m.contains("RFC1918") || m.contains("deny") || m.contains("disallowed"),
                "expected SSRF rejection on hop, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    // ---- Content-Type allowlist. ----

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
            SafeFetchError::InvalidInput(m) => assert!(
                m.contains("application/json"),
                "expected content-type rejection, got: {m}"
            ),
            other => panic!("expected InvalidInput, got {other:?}"),
        }
        handle.abort();
    }

    #[tokio::test]
    async fn fetch_accepts_json_with_charset_param() {
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
