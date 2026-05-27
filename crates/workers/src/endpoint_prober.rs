//! Worker W3 — endpoint_prober.
//!
//! Cadence (vercel.json): every 30 minutes. For every `services[].endpoint`
//! declared in the newest valid manifest for each agent, HEAD-probe (falling
//! back to GET on 405/501) and record a `(status_code, latency_ms, ok)` row
//! in `endpoint_probes`. W7 reads recent probes to compute the liveness
//! component of the trust score.
//!
//! ## Pipeline
//!
//!   1. SELECT one row per (chain_id, agent_id) from `manifests`, taking the
//!      newest `fetched_at` per agent where `validation_status='valid'`.
//!      Decode `parsed` JSONB as [`AgentCard`].
//!   2. Flatten into `(chain_id, agent_id, service_kind, endpoint)` rows.
//!      Skip non-http(s) schemes.
//!   3. Dedup the URL set so each unique endpoint is probed exactly once
//!      per tick; multiple agent rows that reference the same URL get the
//!      same outcome.
//!   4. Run probes 32-way concurrent via `Semaphore::new(32)` + `JoinSet`.
//!      Each probe:
//!        - SSRF pre-vet: resolve host through `tokio::net::lookup_host`,
//!          reject if ANY resolved IP is in [`eth_tools_core::ssrf`].
//!        - Build a `reqwest::Client` pinned to the vetted SocketAddrs via
//!          a `dns::Resolve` implementation (TOCTOU defense).
//!        - HEAD with 5s timeout. If status is 405 or 501, GET with 5s
//!          timeout. GET body is streamed-and-dropped (no buffering).
//!   5. Insert one `endpoint_probes` row per `(agent, service, endpoint)`
//!      tuple. `ok = (200..400).contains(&status_code)`.
//!
//! ## SSRF
//!
//! An agent can register an `agent_uri` that resolves to a benign manifest
//! whose `services[].endpoint` points at `http://169.254.169.254/` (AWS
//! IMDS) or `http://10.0.0.1/`. Without a guard we'd issue that request from
//! our cron worker's network and leak metadata or pivot into a private VPC.
//! [`eth_tools_core::ssrf::is_disallowed_ip`] is the shared deny set; same
//! classifier used by W2 (`manifest_fetcher`) and the MCP `validate_manifest`
//! tool.
//!
//! Closed TOCTOU: we pre-resolve hostname ONCE, vet every returned address,
//! then pin those addresses into the reqwest client via a static resolver.
//! Reqwest never re-resolves, so a hostile authoritative DNS server cannot
//! flip the answer between our vet and the connect.
//!
//! ## No redirect-following
//!
//! Liveness probes follow `redirect::Policy::none()`. A 3xx is recorded as
//! a 3xx (which lands in the `200..400` ok-range, so it's a healthy
//! response). We don't re-vet redirect targets the way `validate_manifest`
//! does because re-probing a different host would conflate liveness of two
//! distinct services.
//!
//! ## Body handling
//!
//! HEAD has no body. The GET fallback uses `client.get(...).send()` which
//! returns headers immediately; we drop the response object without ever
//! calling `.bytes()`, so the body streams nowhere. Response bodies are
//! NEVER logged.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_core::manifest::AgentCard;
use eth_tools_core::ssrf::is_disallowed_ip;
use eth_tools_db::Pool;
use sqlx::Row;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "endpoint_prober";

/// Per-probe wall-clock budget. The 5s figure matches plan §3; HEAD→GET
/// fallback is two sequential attempts so the worst case is ≤ 10s per
/// endpoint, which is well within the 30-minute Vercel cron cadence.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Concurrent in-flight probes. PgBouncer max is 100 connections; reqwest
/// keeps its own connection pool independent of postgres, so 32 is a
/// throughput knob that doesn't push the DB. With 32 concurrent ~300ms
/// probes, ~10k endpoints clears in ~95s.
const CONCURRENCY: usize = 32;

/// One probe target: a unique URL (post-dedup) we'll HEAD/GET once.
#[derive(Debug, Clone)]
struct ProbeTarget {
    endpoint: String,
    // The (agent, service) tuples that reference this URL. Multiple
    // agents declaring the same endpoint share one HTTP request but get
    // distinct `endpoint_probes` rows so per-agent liveness stays
    // accurate.
    refs: Vec<EndpointRef>,
}

#[derive(Debug, Clone)]
struct EndpointRef {
    chain_id: i64,
    agent_id: BigDecimal,
    service_kind: String,
}

/// Per-probe outcome. `status_code = None` means "request never completed"
/// (DNS denied, timeout, transport error); `ok = false` in that case.
#[derive(Debug, Clone)]
struct ProbeOutcome {
    status_code: Option<i32>,
    latency_ms: i32,
    ok: bool,
    error: Option<String>,
}

/// One row queued for `endpoint_probes`. Built from a [`ProbeTarget`]
/// fanned out across its [`EndpointRef`]s.
#[derive(Debug, Clone)]
struct ProbeRow {
    chain_id: i64,
    agent_id: BigDecimal,
    service_name: String,
    endpoint: String,
    probed_at: DateTime<Utc>,
    status_code: Option<i32>,
    latency_ms: i32,
    ok: bool,
    error: Option<String>,
}

/// Worker entrypoint. Wired by `api/cron/endpoint_prober.rs` via
/// `serve_with_context`.
pub async fn run(ctx: WorkerContext) -> Result<WorkerSummary, Error> {
    let pool = ctx.pool.clone();
    let manifests = load_latest_valid_manifests(&pool)
        .await
        .map_err(|e| Error::from(format!("load manifests: {e}")))?;

    let rows_in = manifests.len() as u64;

    // Flatten → dedup. Build a `endpoint -> ProbeTarget` map keyed by URL.
    let mut by_url: BTreeMap<String, ProbeTarget> = BTreeMap::new();
    for m in &manifests {
        let card: AgentCard = match serde_json::from_value(m.parsed.clone()) {
            Ok(c) => c,
            Err(e) => {
                // The DB has `validation_status='valid'` but the JSONB
                // doesn't shape-match — log and move on. We don't drop the
                // whole tick on one bad row.
                tracing::warn!(
                    chain_id = m.chain_id,
                    agent_id = %m.agent_id,
                    error = %e,
                    "manifest parse failed; skipping"
                );
                continue;
            }
        };
        for svc in card.services {
            // Scheme filter: http + https only. Anything else (ipfs://,
            // ws://, mailto:) is out of scope for v1 liveness probing.
            // We tolerate `http` (not just `https`) so the SSRF guard's
            // unit tests at 127.0.0.1 can exercise the probe path.
            let parsed = match url::Url::parse(&svc.endpoint) {
                Ok(u) => u,
                Err(_) => continue,
            };
            if !matches!(parsed.scheme(), "http" | "https") {
                continue;
            }
            let entry = by_url.entry(svc.endpoint.clone()).or_insert_with(|| ProbeTarget {
                endpoint: svc.endpoint.clone(),
                refs: Vec::new(),
            });
            entry.refs.push(EndpointRef {
                chain_id: m.chain_id,
                agent_id: m.agent_id.clone(),
                service_kind: svc.kind.clone(),
            });
        }
    }

    let unique_targets: Vec<ProbeTarget> = by_url.into_values().collect();
    tracing::info!(
        unique_endpoints = unique_targets.len(),
        manifests = manifests.len(),
        "starting probe sweep"
    );

    let sem = Arc::new(Semaphore::new(CONCURRENCY));
    let mut set: JoinSet<(ProbeTarget, ProbeOutcome)> = JoinSet::new();
    for target in unique_targets {
        let sem = sem.clone();
        set.spawn(async move {
            // Owning the permit means semaphore release is RAII even if
            // probe_one panics — JoinSet would still drain.
            let _permit = sem
                .acquire_owned()
                .await
                .expect("semaphore is not closed; w3 owns it");
            let outcome = probe_one(&target.endpoint).await;
            (target, outcome)
        });
    }

    let mut rows: Vec<ProbeRow> = Vec::new();
    let now = Utc::now();
    while let Some(joined) = set.join_next().await {
        let (target, outcome) = match joined {
            Ok(v) => v,
            Err(e) => {
                // JoinError = task panicked or was cancelled. We don't
                // cancel, so panic. Log + continue; per-task panics must
                // not poison the worker.
                tracing::error!(error = %e, "probe task join failed");
                continue;
            }
        };
        for r in &target.refs {
            rows.push(ProbeRow {
                chain_id: r.chain_id,
                agent_id: r.agent_id.clone(),
                service_name: r.service_kind.clone(),
                endpoint: target.endpoint.clone(),
                probed_at: now,
                status_code: outcome.status_code,
                latency_ms: outcome.latency_ms,
                ok: outcome.ok,
                error: outcome.error.clone(),
            });
        }
    }

    let rows_out = rows.len() as u64;

    if ctx.dryrun {
        // Dryrun: count what we WOULD write, never insert. The serve
        // layer suppresses the worker_runs row separately.
        return Ok(WorkerSummary {
            worker: WORKER_NAME,
            ok: true,
            rows_in,
            rows_out,
            dryrun: true,
            skipped: false,
            reason: None,
        });
    }

    insert_probes(&pool, &rows)
        .await
        .map_err(|e| Error::from(format!("insert probes: {e}")))?;

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

/// One row from `manifests` filtered to the newest valid manifest per agent.
#[derive(Debug)]
struct ManifestRow {
    chain_id: i64,
    agent_id: BigDecimal,
    parsed: serde_json::Value,
}

async fn load_latest_valid_manifests(pool: &Pool) -> Result<Vec<ManifestRow>, sqlx::Error> {
    // DISTINCT ON (chain_id, agent_id) ORDER BY (chain_id, agent_id,
    // fetched_at DESC) picks the newest manifest per agent. We further
    // restrict to validation_status='valid' so we don't probe endpoints
    // declared in a malformed manifest.
    //
    // We join against `agents` so we only probe agents currently in the
    // index — a manifest row for a transferred/deregistered agent is no
    // longer relevant to liveness scoring.
    let sql = r#"
        SELECT DISTINCT ON (m.chain_id, m.agent_id)
               m.chain_id, m.agent_id, m.parsed
          FROM manifests m
          JOIN agents a
            ON a.chain_id = m.chain_id AND a.agent_id = m.agent_id
         WHERE m.validation_status = 'valid'
           AND m.parsed IS NOT NULL
         ORDER BY m.chain_id, m.agent_id, m.fetched_at DESC
    "#;
    let rows = sqlx::query(sql).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let chain_id: i64 = r.try_get("chain_id")?;
        let agent_id: BigDecimal = r.try_get("agent_id")?;
        let parsed: serde_json::Value = r.try_get("parsed")?;
        out.push(ManifestRow {
            chain_id,
            agent_id,
            parsed,
        });
    }
    Ok(out)
}

/// Multi-row INSERT into `endpoint_probes`. Doing one INSERT per row would
/// take ~one round-trip each — 10k probes × 5ms RTT ≈ 50s. UNNEST-style
/// array bind is a single round-trip and lets the planner batch the writes.
async fn insert_probes(pool: &Pool, rows: &[ProbeRow]) -> Result<(), sqlx::Error> {
    if rows.is_empty() {
        return Ok(());
    }
    let chain_ids: Vec<i64> = rows.iter().map(|r| r.chain_id).collect();
    let agent_ids: Vec<BigDecimal> = rows.iter().map(|r| r.agent_id.clone()).collect();
    let service_names: Vec<String> = rows.iter().map(|r| r.service_name.clone()).collect();
    let endpoints: Vec<String> = rows.iter().map(|r| r.endpoint.clone()).collect();
    let probed_ats: Vec<DateTime<Utc>> = rows.iter().map(|r| r.probed_at).collect();
    // status_code is NULL-able; build Vec<Option<i32>>.
    let status_codes: Vec<Option<i32>> = rows.iter().map(|r| r.status_code).collect();
    let latency_mss: Vec<i32> = rows.iter().map(|r| r.latency_ms).collect();
    let oks: Vec<bool> = rows.iter().map(|r| r.ok).collect();
    let errors: Vec<Option<String>> = rows.iter().map(|r| r.error.clone()).collect();

    // UNNEST() with explicit type casts. Notes:
    //   * NUMERIC[] needs the explicit cast — sqlx doesn't infer for
    //     BigDecimal arrays.
    //   * Optional INT4 and TEXT need ::INT[] / ::TEXT[] so NULLs survive
    //     the round-trip.
    let sql = r#"
        INSERT INTO endpoint_probes
            (chain_id, agent_id, service_name, endpoint, probed_at,
             status_code, latency_ms, ok, error)
        SELECT * FROM UNNEST(
            $1::BIGINT[],
            $2::NUMERIC[],
            $3::TEXT[],
            $4::TEXT[],
            $5::TIMESTAMPTZ[],
            $6::INT[],
            $7::INT[],
            $8::BOOLEAN[],
            $9::TEXT[]
        )
    "#;
    sqlx::query(sql)
        .bind(&chain_ids)
        .bind(&agent_ids)
        .bind(&service_names)
        .bind(&endpoints)
        .bind(&probed_ats)
        .bind(&status_codes)
        .bind(&latency_mss)
        .bind(&oks)
        .bind(&errors)
        .execute(pool)
        .await?;
    Ok(())
}

// ---- HTTP probe primitives ------------------------------------------------

/// SSRF-vetted HEAD/GET probe of a single URL. Returns a [`ProbeOutcome`]
/// regardless of outcome — errors are captured into the `error` field, not
/// propagated. Callers must record the result either way (a failing probe
/// is itself a liveness signal).
#[doc(hidden)]
pub async fn test_only_probe_one(uri: &str) -> ProbeOutcomeView {
    probe_one(uri).await.into()
}

async fn probe_one(uri: &str) -> ProbeOutcome {
    let start = Instant::now();

    // Parse first — a malformed URL is a permanent failure that doesn't
    // even need DNS.
    let parsed = match url::Url::parse(uri) {
        Ok(u) => u,
        Err(e) => {
            return ProbeOutcome {
                status_code: None,
                latency_ms: 0,
                ok: false,
                error: Some(format!("parse: {e}")),
            }
        }
    };
    let (host, port) = match split_url(&parsed) {
        Ok(v) => v,
        Err(e) => {
            return ProbeOutcome {
                status_code: None,
                latency_ms: 0,
                ok: false,
                error: Some(e),
            }
        }
    };

    // Pre-vet hostname → SocketAddrs. Production resolver only; tests
    // exercise the post-vet HTTP path via `probe_with_client`.
    let addrs = match vet_host(&host, port).await {
        Ok(a) => a,
        Err(e) => {
            return ProbeOutcome {
                status_code: None,
                latency_ms: clamp_ms(start.elapsed()),
                ok: false,
                error: Some(e),
            }
        }
    };

    let client = match build_pinned_client(&host, addrs) {
        Ok(c) => c,
        Err(e) => {
            return ProbeOutcome {
                status_code: None,
                latency_ms: clamp_ms(start.elapsed()),
                ok: false,
                error: Some(e),
            }
        }
    };

    probe_with_client_inner(&client, &parsed, start).await
}

/// HEAD-then-(maybe-)GET attempt against a pre-built `reqwest::Client`.
/// Pulled out so integration tests can drive the HTTP path without going
/// through the SSRF deny set (wiremock binds to 127.0.0.1, which the prod
/// guard rejects pre-flight by design).
///
/// `started` is the wall-clock anchor — `latency_ms` is measured against
/// it so the figure is end-to-end (DNS + connect + request), not just
/// the time spent inside `.send()`.
///
/// Returns a public [`ProbeOutcomeView`] so this seam stays usable from
/// `tests/*` without exposing the worker's internal probe-row struct.
#[doc(hidden)]
pub async fn probe_with_client(
    client: &reqwest::Client,
    url: &url::Url,
    started: Instant,
) -> ProbeOutcomeView {
    probe_with_client_inner(client, url, started).await.into()
}

async fn probe_with_client_inner(
    client: &reqwest::Client,
    url: &url::Url,
    started: Instant,
) -> ProbeOutcome {
    match issue(client, reqwest::Method::HEAD, url).await {
        Ok(status) => {
            // Some origins reject HEAD even when GET works (405 Method
            // Not Allowed, 501 Not Implemented). Fall back to GET in
            // those cases; record the *GET* result so the recorded
            // status reflects what a real client would see.
            if status == 405 || status == 501 {
                match issue(client, reqwest::Method::GET, url).await {
                    Ok(get_status) => ProbeOutcome {
                        status_code: Some(get_status),
                        latency_ms: clamp_ms(started.elapsed()),
                        ok: ok_status(get_status),
                        error: None,
                    },
                    Err(e) => ProbeOutcome {
                        status_code: None,
                        latency_ms: clamp_ms(started.elapsed()),
                        ok: false,
                        error: Some(e),
                    },
                }
            } else {
                ProbeOutcome {
                    status_code: Some(status),
                    latency_ms: clamp_ms(started.elapsed()),
                    ok: ok_status(status),
                    error: None,
                }
            }
        }
        Err(e) => ProbeOutcome {
            status_code: None,
            latency_ms: clamp_ms(started.elapsed()),
            ok: false,
            error: Some(e),
        },
    }
}

/// Build the production-style client (5s timeout, no redirects) but
/// without the SSRF guard or static DNS pin. **Tests only.** Real probes
/// MUST go through `probe_one` so the deny set is enforced.
///
/// Public so the integration test in `tests/endpoint_prober_integration.rs`
/// can construct a client pointing at a `wiremock` listener bound on
/// 127.0.0.1.
#[doc(hidden)]
pub fn test_only_unguarded_client(timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("reqwest builder defaults always succeed")
}

/// Expose [`ProbeOutcome`]'s fields for assertions in integration tests
/// without making the struct public on the worker's API surface.
#[doc(hidden)]
pub struct ProbeOutcomeView {
    pub status_code: Option<i32>,
    pub latency_ms: i32,
    pub ok: bool,
    pub error: Option<String>,
}

impl From<ProbeOutcome> for ProbeOutcomeView {
    fn from(o: ProbeOutcome) -> Self {
        Self {
            status_code: o.status_code,
            latency_ms: o.latency_ms,
            ok: o.ok,
            error: o.error,
        }
    }
}

fn ok_status(s: i32) -> bool {
    (200..400).contains(&s)
}

fn clamp_ms(d: Duration) -> i32 {
    d.as_millis().min(i32::MAX as u128) as i32
}

/// Pull host + port out of a parsed URL, mapping scheme to default port.
fn split_url(url: &url::Url) -> Result<(String, u16), String> {
    let default_port: u16 = match url.scheme() {
        "http" => 80,
        "https" => 443,
        other => return Err(format!("scheme `{other}` not allowed")),
    };
    let host = url
        .host_str()
        .ok_or_else(|| "uri missing host".to_string())?
        .to_string();
    Ok((host, url.port().unwrap_or(default_port)))
}

/// Resolve hostname through tokio DNS, then drop every IP through the
/// shared SSRF deny set. Returns the vetted addresses on success;
/// `Err(reason)` if any address is denied OR resolve returned nothing.
async fn vet_host(host: &str, port: u16) -> Result<Vec<SocketAddr>, String> {
    // Literal IP fast path — `host_str()` strips brackets on `[::1]`
    // so direct parse works for IPv6 too.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_disallowed_ip(ip) {
            return Err(format!("blocked: ip `{ip}` in SSRF deny set"));
        }
        return Ok(vec![SocketAddr::new(ip, port)]);
    }

    // Wrap `lookup_host` in our own timeout — reqwest's per-request
    // timeout starts AFTER vet returns, so without this a single slow
    // authoritative server could pin a probe slot for 30s+ (OS default
    // resolver timeout). 3s is well under PROBE_TIMEOUT so a probe
    // whose DNS times out still fits inside the overall budget.
    let resolved: Vec<SocketAddr> = match tokio::time::timeout(
        Duration::from_secs(3),
        tokio::net::lookup_host((host, port)),
    )
    .await
    {
        Ok(Ok(iter)) => iter.collect(),
        Ok(Err(e)) => return Err(format!("dns resolve `{host}`: {e}")),
        Err(_) => return Err(format!("dns resolve `{host}`: timeout")),
    };
    if resolved.is_empty() {
        return Err(format!("dns resolve `{host}`: no addresses"));
    }
    // If ANY resolved address is private, reject the whole host. A
    // hostile authority that mixes one public + one private answer would
    // otherwise pass the check on the public one and still get probed
    // on the private one if reqwest randomized its choice.
    for addr in &resolved {
        if is_disallowed_ip(addr.ip()) {
            return Err(format!("blocked: `{host}` -> {} in SSRF deny set", addr.ip()));
        }
    }
    Ok(resolved)
}

/// Static `reqwest::dns::Resolve` impl. Pinning vetted addresses into the
/// reqwest client closes the TOCTOU window: reqwest's own connect path
/// never re-resolves, so a hostile authoritative DNS server cannot flip
/// the answer between our `vet_host` call and the actual TCP connect.
struct StaticResolver {
    map: HashMap<String, Vec<SocketAddr>>,
}

impl reqwest::dns::Resolve for StaticResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        type BoxError = Box<dyn std::error::Error + Send + Sync>;
        let key = name.as_str().to_string();
        let addrs = self.map.get(&key).cloned().unwrap_or_default();
        Box::pin(async move {
            let iter: Box<dyn Iterator<Item = SocketAddr> + Send> = Box::new(addrs.into_iter());
            Ok::<_, BoxError>(iter)
        })
    }
}

fn build_pinned_client(host: &str, addrs: Vec<SocketAddr>) -> Result<reqwest::Client, String> {
    let mut map: HashMap<String, Vec<SocketAddr>> = HashMap::new();
    map.insert(host.to_string(), addrs);
    reqwest::Client::builder()
        .timeout(PROBE_TIMEOUT)
        // No redirects: a 3xx is itself the liveness signal; following
        // it could cross hosts and conflate two services' liveness.
        .redirect(reqwest::redirect::Policy::none())
        .dns_resolver(Arc::new(StaticResolver { map }))
        .build()
        .map_err(|e| format!("client build: {e}"))
}

/// Issue one HEAD or GET, returning the status code or a transport-error
/// reason. Body is NOT pulled — we use `.send()` and immediately drop the
/// response. For HEAD this is trivially correct; for GET we never call
/// `.bytes()` or `.text()` so the body chunks go to the OS socket buffer
/// and get GC'd when `Response` drops.
async fn issue(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &url::Url,
) -> Result<i32, String> {
    match client.request(method, url.clone()).send().await {
        Ok(resp) => Ok(resp.status().as_u16() as i32),
        Err(e) if e.is_timeout() => Err("timeout".to_string()),
        Err(e) if e.is_connect() => Err(format!("connect: {}", e)),
        Err(e) => Err(format!("http: {}", e)),
    }
}

// ---- Pure-fn helpers exposed for unit testing -----------------------------

/// Extract the unique-by-URL probe set out of an `AgentCard`. Pure: no
/// I/O, no SSRF check. Unit-testable.
///
/// Returns `Vec<(service_kind, endpoint)>` with duplicates collapsed by
/// URL — the caller picks how to attribute the probe back to agents.
#[doc(hidden)]
pub fn extract_probable_endpoints(card: &AgentCard) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for svc in &card.services {
        let Ok(u) = url::Url::parse(&svc.endpoint) else {
            continue;
        };
        if !matches!(u.scheme(), "http" | "https") {
            continue;
        }
        if seen.insert(svc.endpoint.clone()) {
            out.push((svc.kind.clone(), svc.endpoint.clone()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use eth_tools_core::manifest::{AgentCard, Service};

    fn card_with(services: Vec<(&str, &str)>) -> AgentCard {
        AgentCard {
            name: "t".into(),
            description: None,
            services: services
                .into_iter()
                .map(|(k, e)| Service {
                    kind: k.into(),
                    endpoint: e.into(),
                })
                .collect(),
        }
    }

    #[test]
    fn extract_dedupes_by_url() {
        let card = card_with(vec![
            ("web", "https://a.example/x"),
            ("A2A", "https://a.example/x"),
            ("MCP", "https://b.example/m"),
        ]);
        let out = extract_probable_endpoints(&card);
        assert_eq!(out.len(), 2);
        let urls: BTreeSet<&str> = out.iter().map(|(_, u)| u.as_str()).collect();
        assert!(urls.contains("https://a.example/x"));
        assert!(urls.contains("https://b.example/m"));
    }

    #[test]
    fn extract_skips_non_http_schemes() {
        let card = card_with(vec![
            ("web", "ipfs://QmFoo"),
            ("email", "mailto:agent@x"),
            ("ENS", "ens://name.eth"),
            ("MCP", "wss://w.example/ws"),
            ("web", "https://good.example/api"),
            ("OASF", "http://plain.example/oasf"),
        ]);
        let out = extract_probable_endpoints(&card);
        // Only http + https survive.
        assert_eq!(out.len(), 2);
        let urls: BTreeSet<&str> = out.iter().map(|(_, u)| u.as_str()).collect();
        assert!(urls.contains("https://good.example/api"));
        assert!(urls.contains("http://plain.example/oasf"));
    }

    #[test]
    fn extract_skips_unparseable_urls() {
        let card = card_with(vec![
            ("web", "not a url"),
            ("web", "://broken"),
            ("MCP", "https://ok.example/"),
        ]);
        let out = extract_probable_endpoints(&card);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1, "https://ok.example/");
    }

    #[test]
    fn ok_status_band() {
        assert!(ok_status(200));
        assert!(ok_status(204));
        assert!(ok_status(301));
        assert!(ok_status(399));
        assert!(!ok_status(400));
        assert!(!ok_status(404));
        assert!(!ok_status(500));
        assert!(!ok_status(599));
        assert!(!ok_status(199));
    }

    #[test]
    fn split_url_classifies_schemes() {
        let u = url::Url::parse("https://x:8443/p").unwrap();
        assert_eq!(split_url(&u).unwrap(), ("x".into(), 8443));
        let u = url::Url::parse("http://x/p").unwrap();
        assert_eq!(split_url(&u).unwrap(), ("x".into(), 80));
        let u = url::Url::parse("https://x/p").unwrap();
        assert_eq!(split_url(&u).unwrap(), ("x".into(), 443));
        let u = url::Url::parse("ftp://x/p").unwrap();
        assert!(split_url(&u).is_err());
    }

    #[tokio::test]
    async fn vet_host_rejects_loopback_literal() {
        let e = vet_host("127.0.0.1", 80).await.unwrap_err();
        assert!(e.contains("SSRF deny set") || e.contains("blocked"), "got: {e}");
    }

    #[tokio::test]
    async fn vet_host_rejects_imds_literal() {
        let e = vet_host("169.254.169.254", 80).await.unwrap_err();
        assert!(e.contains("deny set") || e.contains("blocked"), "got: {e}");
    }

    #[tokio::test]
    async fn vet_host_rejects_ipv4_mapped_loopback() {
        let e = vet_host("::ffff:127.0.0.1", 80).await.unwrap_err();
        assert!(e.contains("deny set") || e.contains("blocked"), "got: {e}");
    }

    #[tokio::test]
    async fn vet_host_allows_public_literal() {
        // Pure deny-set check on the fast path; no DNS.
        let addrs = vet_host("8.8.8.8", 443).await.unwrap();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].port(), 443);
    }

    #[tokio::test]
    async fn probe_one_rejects_loopback_url_pre_flight() {
        // The prod guard MUST reject before any HTTP is issued. No mock
        // listener — if a connect attempt happens, the test'd hang or
        // hit "connection refused".
        let out = probe_one("http://127.0.0.1:1/health").await;
        assert!(!out.ok);
        assert!(out.status_code.is_none());
        let err = out.error.unwrap();
        assert!(err.contains("blocked") || err.contains("deny"), "got: {err}");
    }
}
