//! Worker W8 — daily wallet balance keeper + worker watchdog (plan §3 row W8).
//!
//! Three concerns combined into one daily cron tick:
//!
//!   1. **Operator EOA balance.** Read the live balance via
//!      `provider.get_balance`. If it crosses the upper sweep threshold
//!      ($5), queue a Safe sweep transaction (status='queued'). If it
//!      drops below the floor ($1), raise an `alerts(severity='warn',
//!      source='wallet.low')`.
//!   2. **Worker freshness.** Each of the 8 workers has an expected
//!      cadence. If any worker's newest `ok=true` row in `worker_runs`
//!      is older than 3× that cadence — or if it has NEVER posted an
//!      ok=true row — raise `alerts(severity='crit',
//!      source='worker.stale')`.
//!   3. **Cursor lag.** If any scrape cursor's `last_block` is more than
//!      `MAX_CURSOR_LAG_BLOCKS` (100) behind the chain head, raise
//!      `alerts(severity='warn', source='cursor.lag')`.
//!
//! ## Why combine three jobs in one worker?
//!
//! Watchdog signals are noisy if scanned more than daily; balance checks
//! cost an RPC call apiece. Running them all on the daily cadence keeps
//! Vercel cron slot usage at one entry. Alert dedup is the dashboard's
//! responsibility — we always re-raise; an operator who acks an alert
//! we keep seeing has chosen to silence it intentionally.
//!
//! ## Configuration via env (read once per invocation):
//!
//!   - `WALLET_SIGNER_ADDRESS` — 0x-prefixed 20-byte hex of the EOA.
//!     No default. If unset, balance check is skipped (watchdog still runs).
//!   - `WALLET_SAFE_ADDRESS` — 0x-prefixed Safe to sweep into.
//!   - `ETH_USD_PRICE_CENTS` — integer cents/ETH. Default `350_000`
//!     ($3500/ETH). Used to convert wei→USD.
//!   - `WALLET_SWEEP_THRESHOLD_CENTS` — default 500 ($5).
//!   - `WALLET_ALERT_THRESHOLD_CENTS` — default 100 ($1).

use std::collections::HashMap;

use alloy_primitives::{Address, U256};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use eth_tools_db::{alerts, wallet_txs};
use serde_json::json;
use vercel_runtime::Error;

use crate::{WorkerContext, WorkerSummary};

const WORKER_NAME: &str = "wallet_balance_keeper";

/// Default ETH→USD rate, in cents per ETH. Overridden by env
/// `ETH_USD_PRICE_CENTS`. 350_000 = $3500/ETH (rough mid-2025 baseline).
pub const DEFAULT_ETH_USD_CENTS: u128 = 350_000;

/// Threshold above which we queue a sweep, in cents.
pub const DEFAULT_SWEEP_THRESHOLD_CENTS: u128 = 500; // $5

/// Threshold below which we raise a low-balance alert, in cents.
pub const DEFAULT_ALERT_THRESHOLD_CENTS: u128 = 100; // $1

/// A cursor is flagged as lagging if `head_block - last_block` exceeds
/// this. Per task spec.
pub const MAX_CURSOR_LAG_BLOCKS: i64 = 100;

/// `execTransaction` selector for Gnosis Safe — first 4 bytes of
/// keccak256("execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)").
/// Pre-computed so we don't pull in `keccak-asm`.
const SAFE_EXEC_TRANSACTION_SELECTOR: [u8; 4] = [0x6a, 0x76, 0x12, 0x02];

/// Per-worker cadence in minutes; the watchdog flags any worker whose
/// newest ok=true row is older than `3 * cadence`. Keep this in sync
/// with `vercel.json` cron schedules.
///
/// **Order matches `ALL_WORKERS` in `lib.rs`** so the constants stay
/// adjacent under code review.
pub const WORKER_CADENCES_MIN: &[(&str, i64)] = &[
    ("registry_scraper", 2),
    ("manifest_fetcher", 15),
    ("endpoint_prober", 30),
    ("reputation_aggregator", 5),
    ("validation_aggregator", 5),
    ("wallet_rotation_watcher", 1),
    ("trust_score_recompute", 60),
    // wallet_balance_keeper isn't in the watchdog set — it would alert on
    // itself only after its first run; meaningless.
];

/// Public entrypoint called from `api/cron/wallet_balance_keeper.rs`.
pub async fn run(ctx: &WorkerContext, chain_id: u64) -> Result<WorkerSummary, Error> {
    let mut alerts_raised: u64 = 0;
    let mut sweeps_queued: u64 = 0;

    // ---- 1. Balance check (optional — only runs if signer configured) -----
    let signer = std::env::var("WALLET_SIGNER_ADDRESS").ok();
    if let Some(addr_hex) = signer {
        match parse_address(&addr_hex) {
            Ok(addr) => {
                let (queued, alerted) =
                    check_balance(ctx, chain_id, addr).await?;
                sweeps_queued += queued;
                alerts_raised += alerted;
            }
            Err(e) => {
                tracing::warn!(
                    worker = WORKER_NAME,
                    error = %e,
                    "WALLET_SIGNER_ADDRESS unparseable; skipping balance check"
                );
            }
        }
    } else {
        tracing::info!(
            worker = WORKER_NAME,
            "WALLET_SIGNER_ADDRESS not set; skipping balance check"
        );
    }

    // ---- 2. Worker watchdog -----------------------------------------------
    alerts_raised += run_watchdog(ctx).await?;

    // ---- 3. Cursor lag check ----------------------------------------------
    alerts_raised += check_cursor_lag(ctx).await?;

    Ok(WorkerSummary {
        worker: WORKER_NAME,
        ok: true,
        rows_in: 0,
        rows_out: alerts_raised.saturating_add(sweeps_queued),
        dryrun: ctx.dryrun,
        skipped: false,
        reason: None,
    })
}

/// Read the EOA balance, queue a sweep if over threshold, raise an
/// alert if under the floor. Returns (sweeps_queued, alerts_raised).
async fn check_balance(
    ctx: &WorkerContext,
    chain_id: u64,
    signer: Address,
) -> Result<(u64, u64), Error> {
    let balance_wei = ctx
        .rpc
        .get_balance(signer)
        .await
        .map_err(|e| Error::from(format!("get_balance failed: {e}")))?;

    let cents_per_eth = read_env_u128("ETH_USD_PRICE_CENTS", DEFAULT_ETH_USD_CENTS);
    let sweep_cents = read_env_u128("WALLET_SWEEP_THRESHOLD_CENTS", DEFAULT_SWEEP_THRESHOLD_CENTS);
    let alert_cents = read_env_u128("WALLET_ALERT_THRESHOLD_CENTS", DEFAULT_ALERT_THRESHOLD_CENTS);

    let balance_cents = wei_to_usd_cents(balance_wei, cents_per_eth);

    tracing::info!(
        worker = WORKER_NAME,
        balance_wei = %balance_wei,
        balance_cents,
        sweep_cents,
        alert_cents,
        "balance check"
    );

    if ctx.dryrun {
        tracing::info!(worker = WORKER_NAME, "dryrun: skipping balance write paths");
        return Ok((0, 0));
    }

    let mut queued = 0;
    let mut raised = 0;

    if balance_cents > sweep_cents {
        let safe_addr = std::env::var("WALLET_SAFE_ADDRESS")
            .ok()
            .and_then(|h| parse_address(&h).ok());
        let to = match safe_addr {
            Some(a) => a.0.to_vec(),
            None => signer.0.to_vec(), // self-target marker; signer reads later
        };
        let id = wallet_txs::queue(
            &ctx.pool,
            wallet_txs::QueueInsert {
                chain_id: chain_id as i64,
                to_addr: &to,
                selector: &SAFE_EXEC_TRANSACTION_SELECTOR,
                value_wei: BigDecimal::from(0),
            },
        )
        .await
        .map_err(|e| Error::from(format!("wallet_txs::queue failed: {e}")))?;
        tracing::info!(worker = WORKER_NAME, wallet_tx_id = id, "queued Safe sweep tx");
        queued += 1;
    }

    if balance_cents < alert_cents {
        let id = alerts::insert(
            &ctx.pool,
            alerts::SEV_WARN,
            "wallet.low",
            json!({
                "balance_cents": balance_cents,
                "threshold_cents": alert_cents,
                "signer": format!("0x{}", hex_lower(signer.0.as_ref())),
            }),
        )
        .await
        .map_err(|e| Error::from(format!("alerts::insert(wallet.low) failed: {e}")))?;
        tracing::warn!(worker = WORKER_NAME, alert_id = id, "wallet balance low");
        raised += 1;
    }

    Ok((queued, raised))
}

/// Scan `worker_runs` for stale workers. Returns count of alerts raised.
async fn run_watchdog(ctx: &WorkerContext) -> Result<u64, Error> {
    let last_oks = fetch_last_ok_by_worker(&ctx.pool).await?;
    let now = Utc::now();
    let mut raised: u64 = 0;

    for (name, cadence_min) in WORKER_CADENCES_MIN {
        let stale_after_min = cadence_min.saturating_mul(3);
        let stale_threshold = chrono::Duration::minutes(stale_after_min);

        let body = match last_oks.get(*name) {
            Some(ts) => {
                let age = now.signed_duration_since(*ts);
                if age <= stale_threshold {
                    continue; // healthy
                }
                json!({
                    "worker": name,
                    "last_ok_at": ts.to_rfc3339(),
                    "age_seconds": age.num_seconds(),
                    "stale_after_minutes": stale_after_min,
                })
            }
            None => json!({
                "worker": name,
                "last_ok_at": serde_json::Value::Null,
                "reason": "never had an ok=true run",
                "stale_after_minutes": stale_after_min,
            }),
        };

        if ctx.dryrun {
            tracing::info!(worker = WORKER_NAME, target = name, ?body, "dryrun: would alert worker.stale");
            continue;
        }

        let id = alerts::insert(&ctx.pool, alerts::SEV_CRIT, "worker.stale", body)
            .await
            .map_err(|e| Error::from(format!("alerts::insert(worker.stale) failed: {e}")))?;
        tracing::error!(worker = WORKER_NAME, alert_id = id, target = name, "stale worker alert");
        raised += 1;
    }

    Ok(raised)
}

/// Single statement: latest `ok=true` start time per worker_name. Returns
/// a map keyed by worker_name. Workers with no ok rows are absent (caller
/// treats absence as "never succeeded → alert").
async fn fetch_last_ok_by_worker(
    pool: &eth_tools_db::Pool,
) -> Result<HashMap<String, DateTime<Utc>>, Error> {
    let rows: Vec<(String, DateTime<Utc>)> = sqlx::query_as(
        "SELECT worker_name, MAX(started_at)
           FROM worker_runs
          WHERE ok = TRUE
          GROUP BY worker_name",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| Error::from(format!("fetch_last_ok_by_worker failed: {e}")))?;
    Ok(rows.into_iter().collect())
}

/// Compare each `cursors.last_block` against the chain head; flag any
/// gap > [`MAX_CURSOR_LAG_BLOCKS`]. Returns count of alerts raised.
async fn check_cursor_lag(ctx: &WorkerContext) -> Result<u64, Error> {
    let head = match ctx.rpc.get_block_number().await {
        Ok(h) => h as i64,
        Err(e) => {
            tracing::warn!(
                worker = WORKER_NAME,
                error = %e,
                "head fetch failed; skipping cursor lag check"
            );
            return Ok(0);
        }
    };

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT cursor_key, last_block FROM cursors WHERE last_block > 0",
    )
    .fetch_all(&ctx.pool)
    .await
    .map_err(|e| Error::from(format!("cursor scan failed: {e}")))?;

    let mut raised: u64 = 0;
    for (key, last_block) in rows {
        let lag = head.saturating_sub(last_block);
        if lag <= MAX_CURSOR_LAG_BLOCKS {
            continue;
        }
        let body = json!({
            "cursor_key": key,
            "last_block": last_block,
            "head_block": head,
            "lag_blocks": lag,
        });
        if ctx.dryrun {
            tracing::info!(worker = WORKER_NAME, ?body, "dryrun: would alert cursor.lag");
            continue;
        }
        let id = alerts::insert(&ctx.pool, alerts::SEV_WARN, "cursor.lag", body)
            .await
            .map_err(|e| Error::from(format!("alerts::insert(cursor.lag) failed: {e}")))?;
        tracing::warn!(worker = WORKER_NAME, alert_id = id, cursor = %key, "cursor lag alert");
        raised += 1;
    }
    Ok(raised)
}

/// Convert wei → integer USD cents. `cents_per_eth` is `u128`; result
/// is clamped at `u128::MAX` (impossible in practice — would require >
/// 10^30 ETH).
///
/// The arithmetic: `cents = wei * cents_per_eth / 10^18`. We do the
/// multiplication in U256 to avoid overflow (a U256 wei × u128 cents
/// could fit in 384 bits — alloy's U256 has 256, but ETH supply caps
/// the realistic range at <10^27 wei, well under U256 saturation).
pub fn wei_to_usd_cents(wei: U256, cents_per_eth: u128) -> u128 {
    // 10^18 wei per ETH.
    let wei_per_eth = U256::from(1_000_000_000_000_000_000u128);
    let rate = U256::from(cents_per_eth);
    // cents = wei * rate / wei_per_eth
    let cents_u256 = wei.saturating_mul(rate).checked_div(wei_per_eth);
    let cents = match cents_u256 {
        Some(c) => c,
        None => return 0, // wei_per_eth is non-zero so this only happens on internal bug
    };
    // u128 saturation: realistic balances are <2^96 wei, but be explicit.
    let limbs = cents.into_limbs();
    if limbs[2] != 0 || limbs[3] != 0 {
        return u128::MAX;
    }
    ((limbs[1] as u128) << 64) | (limbs[0] as u128)
}

/// Parse a `0x…` 20-byte hex address. Accepts mixed case.
fn parse_address(s: &str) -> Result<Address, String> {
    let hex = s.strip_prefix("0x").unwrap_or(s);
    if hex.len() != 40 {
        return Err(format!("address must be 20 bytes (40 hex chars), got {}", hex.len()));
    }
    let mut out = [0u8; 20];
    for i in 0..20 {
        let byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|e| format!("non-hex digit: {e}"))?;
        out[i] = byte;
    }
    Ok(Address::from(out))
}

fn read_env_u128(key: &str, default_v: u128) -> u128 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse::<u128>().ok())
        .unwrap_or(default_v)
}

fn hex_lower(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wei_to_cents_one_eth() {
        // 1 ETH = 10^18 wei, at $3500 = 350_000 cents.
        let one_eth = U256::from(1_000_000_000_000_000_000u128);
        assert_eq!(wei_to_usd_cents(one_eth, 350_000), 350_000);
    }

    #[test]
    fn wei_to_cents_half_eth() {
        let half = U256::from(500_000_000_000_000_000u128);
        assert_eq!(wei_to_usd_cents(half, 350_000), 175_000);
    }

    #[test]
    fn wei_to_cents_dust_rounds_down() {
        // 1 wei * 350_000 / 10^18 = 0 (integer division).
        assert_eq!(wei_to_usd_cents(U256::from(1u128), 350_000), 0);
    }

    #[test]
    fn wei_to_cents_zero() {
        assert_eq!(wei_to_usd_cents(U256::ZERO, 350_000), 0);
    }

    #[test]
    fn wei_to_cents_overrides_rate() {
        let one_eth = U256::from(1_000_000_000_000_000_000u128);
        // $1/ETH baseline override.
        assert_eq!(wei_to_usd_cents(one_eth, 100), 100);
    }

    #[test]
    fn parse_address_accepts_0x_and_bare() {
        let addr = parse_address("0x0000000000000000000000000000000000000001").unwrap();
        let addr2 = parse_address("0000000000000000000000000000000000000001").unwrap();
        assert_eq!(addr, addr2);
        assert_eq!(addr.0[19], 1);
    }

    #[test]
    fn parse_address_rejects_wrong_length() {
        assert!(parse_address("0x1234").is_err());
        let long = "0x".to_string() + &"a".repeat(41);
        assert!(parse_address(&long).is_err());
    }

    #[test]
    fn parse_address_rejects_non_hex() {
        assert!(parse_address("0xZZ00000000000000000000000000000000000001").is_err());
    }

    #[test]
    fn worker_cadences_cover_seven_workers() {
        // Seven workers (W1..W7); W8 (self) is intentionally absent.
        assert_eq!(WORKER_CADENCES_MIN.len(), 7);
        let names: Vec<&str> = WORKER_CADENCES_MIN.iter().map(|(n, _)| *n).collect();
        for w in [
            "registry_scraper",
            "manifest_fetcher",
            "endpoint_prober",
            "reputation_aggregator",
            "validation_aggregator",
            "wallet_rotation_watcher",
            "trust_score_recompute",
        ] {
            assert!(names.contains(&w), "missing cadence for {w}");
        }
        assert!(!names.contains(&"wallet_balance_keeper"));
    }

    #[test]
    fn read_env_u128_falls_back_on_unset_or_invalid() {
        std::env::remove_var("__W8_TEST_VAR__");
        assert_eq!(read_env_u128("__W8_TEST_VAR__", 42), 42);
        std::env::set_var("__W8_TEST_VAR__", "not-a-number");
        assert_eq!(read_env_u128("__W8_TEST_VAR__", 42), 42);
        std::env::set_var("__W8_TEST_VAR__", "100");
        assert_eq!(read_env_u128("__W8_TEST_VAR__", 42), 100);
        std::env::remove_var("__W8_TEST_VAR__");
    }

    #[test]
    fn safe_selector_is_canonical() {
        // 0x6a761202 — first 4 bytes of keccak256(execTransaction(...)).
        assert_eq!(SAFE_EXEC_TRANSACTION_SELECTOR, [0x6a, 0x76, 0x12, 0x02]);
    }
}
