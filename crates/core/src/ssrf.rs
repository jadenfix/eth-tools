//! Shared SSRF (Server-Side Request Forgery) IP deny-set classifier.
//!
//! Workers that follow user-controlled URLs (manifest_fetcher → manifest body
//! fetch; endpoint_prober → service liveness probe) MUST run every resolved
//! IP through [`is_disallowed_ip`] before opening a TCP connection. An
//! attacker who controls an `agent_uri` or `services[].endpoint` could
//! otherwise coerce eth-tools into hitting:
//!
//!   - EC2/IMDS (`169.254.169.254`) → AWS credential exfiltration.
//!   - GCP metadata (`169.254.169.254` via link-local) → same.
//!   - Internal RFC 1918 services (`10.0.0.0/8`, `172.16/12`, `192.168/16`)
//!     → reach into the deployer's private network.
//!   - Loopback (`127.0.0.1`, `::1`) → reach into colocated services.
//!
//! ## Why not depend on the mcp copy?
//!
//! `crates/mcp` carries an identical classifier inside `tools/manifest.rs`
//! (Issue 1/4 of Phase-5 deep review). That copy is private (`fn`, not
//! `pub fn`) and embedded in the MCP tool's fetch machinery. Promoting it
//! through mcp's public surface would tightly couple two crates that
//! otherwise have no reason to know about each other. The classifier is
//! pure (no I/O, no state), so duplicating ~50 lines of `match` arms in
//! `core` is the lower-coupling tradeoff. Both copies are unit-tested.
//!
//! ## Why is this in `core`, not in `workers`?
//!
//! Multiple downstream callers will need it (W2 manifest_fetcher, W3
//! endpoint_prober, future tools that resolve user-provided URIs). Putting
//! it in `core` lets every consumer share one deny set; otherwise a worker
//! that forgets to dedupe will quietly become an exploit vector.

use std::net::IpAddr;

/// Returns `true` if `ip` is in the deny set: an attacker who steers our
/// fetcher at this address can leak cloud metadata, hit internal services,
/// or pivot through tunneled wrappers. See module docs for the threat model.
///
/// Deny set:
///   * RFC 1918 private (10/8, 172.16/12, 192.168/16) via `is_private`
///   * Loopback (127/8, ::1)
///   * Link-local (169.254/16, fe80::/10) — covers EC2/IMDS
///   * Unspecified (0.0.0.0, ::) and the `0.0.0.0/8` "this network" block
///   * IPv4-mapped IPv6 (`::ffff:a.b.c.d`) — re-checked as IPv4
///   * IPv6 ULA (fc00::/7)
///   * IPv6 6to4 (2002::/16) — denied wholesale; even 6to4-of-public is
///     suspect because 6to4 routing has known bypass anomalies
///   * IPv6 Teredo (2001::/32) — denied; obscure tunnel rarely used
///     legitimately, often used to bypass IPv4 filters.
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    // Normalize IPv4-mapped IPv6 to IPv4 first so a single check covers
    // both `127.0.0.1` and `::ffff:127.0.0.1`.
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
        // fc00::/7 — Unique Local Addresses.
        if (s[0] & 0xfe00) == 0xfc00 {
            return true;
        }
        // fe80::/10 — link-local.
        if (s[0] & 0xffc0) == 0xfe80 {
            return true;
        }
        // 2002::/16 — 6to4. Embedded IPv4 lives in segments [1..3]
        // (the next 32 bits). Unwrap and re-check so 6to4-of-private
        // is denied; then deny the whole 6to4 prefix anyway because
        // 6to4 routing anomalies have been abused.
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
            return true;
        }
        // 2001::/32 — Teredo.
        if s[0] == 0x2001 && s[1] == 0x0000 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deny_set_classifications() {
        for ip in [
            "10.0.0.1",
            "172.16.0.1",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "0.0.0.0",
            "0.1.2.3",
            "::1",
            "::",
            "fc00::1",
            "fd00::1",
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
    fn ipv4_mapped_loopback_denied() {
        // ::ffff:127.0.0.1 — Ipv6Addr::is_loopback returns false; the
        // explicit to_ipv4_mapped path is the one catching it.
        let ip: IpAddr = "::ffff:127.0.0.1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[test]
    fn six_to_four_of_loopback_denied() {
        let ip: IpAddr = "2002:7f00:0001::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[test]
    fn six_to_four_of_public_denied() {
        // Conservative: even 6to4 of a public IPv4 is denied.
        let ip: IpAddr = "2002:0808:0808::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }

    #[test]
    fn teredo_denied() {
        let ip: IpAddr = "2001:0:0:0::1".parse().unwrap();
        assert!(is_disallowed_ip(ip));
    }
}
