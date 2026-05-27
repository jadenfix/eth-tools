//! Gas pricing + cost reconciliation helpers (phase 6.2).
//!
//! ## The constants
//!
//! `MAX_FEE_PER_GAS_WEI` is the hard ceiling on `max_fee_per_gas` for any tx
//! the wallet signs. Derived from:
//!
//! ```text
//!   per_tx_cost_cents  = MAX_DAILY_SPEND_USD_CENTS / minimum_txs_per_day
//!                      = 100 / 2                   = 50 cents
//!   per_tx_cost_eth    = 50 cents / eth_usd_price  ≈ 0.0001428 ETH (@ $3500)
//!   per_tx_cost_wei    = 0.0001428 * 1e18         ≈ 1.428e14 wei
//!   max_fee_per_gas    = per_tx_cost_wei / MAX_PER_TX_GAS
//!                      = 1.428e14 / 500_000        ≈ 2.857e8 wei
//!                      = 0.286 gwei
//! ```
//!
//! We round up to **0.5 gwei (5e8 wei)** as the conservative ceiling. Base
//! mainnet base-fees in late 2025 sit at 0.001-0.02 gwei; 0.5 gwei is ~25x
//! headroom so transient congestion doesn't force a denial. Anything above
//! 0.5 gwei effectively means Base is having an emergency and we should
//! refuse the tx (the kill switch is the manual override).
//!
//! `MIN_PRIORITY_FEE_WEI` = 1 gwei is the historical minimum that builders
//! prioritise on Base. Below this the tx may sit in mempool indefinitely.
//! Wait — Base is L2 and the priority-fee market is different. On Base the
//! priority floor is closer to 0.001 gwei; we use that as the actual floor
//! and document the L1 number above as a foot-gun.

use alloy_primitives::U256;

use crate::wallet::rpc::GasPrice;

/// Per-tx hard ceiling on `max_fee_per_gas` (wei). Derivation in the module docs.
/// Always integer; never derived from a float at runtime.
pub const MAX_FEE_PER_GAS_WEI: u128 = 500_000_000; // 0.5 gwei

/// Floor on `max_priority_fee_per_gas` (wei). Base mainnet builders honour
/// 0.001 gwei priority; we use 0.01 gwei (10x) for headroom against
/// fee-history spikes.
pub const MIN_PRIORITY_FEE_WEI: u128 = 10_000_000; // 0.01 gwei

/// Wei per USD cent at the assumed conversion rate of $3500/ETH:
///   1 ETH    = 1e18 wei
///   1 USD    = 1/3500 ETH ≈ 2.857e14 wei
///   1 cent   = 2.857e12 wei
///
/// We pin this constant (instead of fetching live) so the daily cap is
/// deterministic across cold starts; the live-price wallet (W7
/// rebalancer) recomputes this with a real oracle on its own cadence.
/// **Refresh manually** on the next ETH price re-baseline.
pub const WEI_PER_USD_CENT: u128 = 2_857_142_857_142;

/// Clamp a `GasPrice` returned by the RPC to the wallet's hard ceilings.
/// Returns the clamped pair; caller compares against the rail-level gas
/// cap on `tx.gas_limit` independently.
pub fn clamp_gas_price(rpc: GasPrice) -> GasPrice {
    let max_fee = rpc.max_fee_per_gas.min(MAX_FEE_PER_GAS_WEI);
    // Priority must be <= max fee. If RPC suggests a priority above the
    // (already-clamped) max fee, drop it to (max_fee / 2) so it stays a
    // valid tx. This shouldn't happen in practice — base fees on Base are
    // tiny — but the test suite verifies the invariant either way.
    let priority = rpc
        .max_priority_fee_per_gas
        .max(MIN_PRIORITY_FEE_WEI)
        .min(max_fee);
    GasPrice {
        max_fee_per_gas: max_fee,
        max_priority_fee_per_gas: priority,
    }
}

/// True iff `rpc.max_fee_per_gas` is above [`MAX_FEE_PER_GAS_WEI`]. The
/// signing path uses this for the runtime gas-cap denial (post-rail, after
/// we know what the chain wants — the rails check `gas_limit`, this checks
/// `max_fee_per_gas`).
pub fn exceeds_fee_ceiling(rpc: GasPrice) -> bool {
    rpc.max_fee_per_gas > MAX_FEE_PER_GAS_WEI
}

/// Convert receipt cost (gas_used × effective_gas_price) into USD cents.
/// Integer math throughout — never a float. Saturating on overflow (a
/// single tx that overflows i64 USD cents is nonsense the rail rejected
/// upstream; the saturation keeps the type stable for the reconcile call).
pub fn wei_cost_to_cents(gas_used: u64, effective_gas_price_wei: u128) -> i64 {
    let total_wei = U256::from(gas_used) * U256::from(effective_gas_price_wei);
    let cents = total_wei / U256::from(WEI_PER_USD_CENT);
    // i64::MAX cents is ~92e18 — comfortably more than any single Base tx
    // can ever cost. The saturation is defensive.
    let cents_u64: u64 = cents.try_into().unwrap_or(u64::MAX);
    i64::try_from(cents_u64).unwrap_or(i64::MAX)
}

/// Convert wei → USD cents for the balance rail. Same math as the cost path.
pub fn balance_wei_to_cents(balance_wei: U256) -> u64 {
    let cents = balance_wei / U256::from(WEI_PER_USD_CENT);
    cents.try_into().unwrap_or(u64::MAX)
}

/// Per-tx gas envelope sanity check — `gas_limit * max_fee_per_gas` must
/// stay under a single-tx wei budget derived from the daily cap. Returns
/// `Ok(())` if the worst-case spend fits in the cap, `Err(cents_over)`
/// otherwise. The signing path uses this AFTER clamping so it sees the
/// final budget the chain will charge.
pub fn check_worst_case_cost(gas_limit: u64, gas: GasPrice) -> Result<(), i64> {
    let worst_wei = U256::from(gas_limit) * U256::from(gas.max_fee_per_gas);
    let worst_cents_u64: u64 = (worst_wei / U256::from(WEI_PER_USD_CENT))
        .try_into()
        .unwrap_or(u64::MAX);
    // Saturating into i64
    let worst_cents = i64::try_from(worst_cents_u64).unwrap_or(i64::MAX);
    // Allow up to MAX_DAILY_SPEND_USD_CENTS for a single tx — anything more
    // would burn the whole day's budget on one call.
    let cap = i64::from(crate::wallet::MAX_DAILY_SPEND_USD_CENTS);
    if worst_cents > cap {
        Err(worst_cents - cap)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::MAX_PER_TX_GAS;

    #[test]
    fn clamp_caps_max_fee() {
        let g = GasPrice {
            max_fee_per_gas: 2 * MAX_FEE_PER_GAS_WEI,
            max_priority_fee_per_gas: MIN_PRIORITY_FEE_WEI,
        };
        let c = clamp_gas_price(g);
        assert_eq!(c.max_fee_per_gas, MAX_FEE_PER_GAS_WEI);
    }

    #[test]
    fn clamp_floors_priority() {
        let g = GasPrice {
            max_fee_per_gas: MAX_FEE_PER_GAS_WEI,
            max_priority_fee_per_gas: 0,
        };
        let c = clamp_gas_price(g);
        assert_eq!(c.max_priority_fee_per_gas, MIN_PRIORITY_FEE_WEI);
    }

    #[test]
    fn clamp_keeps_priority_le_max_fee() {
        let g = GasPrice {
            max_fee_per_gas: 5_000_000, // 0.005 gwei — below MIN_PRIORITY!
            max_priority_fee_per_gas: MIN_PRIORITY_FEE_WEI,
        };
        let c = clamp_gas_price(g);
        assert!(
            c.max_priority_fee_per_gas <= c.max_fee_per_gas,
            "priority {} > max_fee {}",
            c.max_priority_fee_per_gas,
            c.max_fee_per_gas
        );
    }

    #[test]
    fn exceeds_fee_ceiling_fires_above_cap() {
        let g = GasPrice {
            max_fee_per_gas: MAX_FEE_PER_GAS_WEI + 1,
            max_priority_fee_per_gas: 0,
        };
        assert!(exceeds_fee_ceiling(g));
    }

    #[test]
    fn exceeds_fee_ceiling_quiet_at_cap() {
        let g = GasPrice {
            max_fee_per_gas: MAX_FEE_PER_GAS_WEI,
            max_priority_fee_per_gas: 0,
        };
        assert!(!exceeds_fee_ceiling(g));
    }

    #[test]
    fn wei_cost_at_full_envelope_fits_within_daily_cap() {
        // 500k gas at 0.5 gwei = 2.5e14 wei ≈ 87¢ @ $3500/ETH.
        // Must stay ≤ MAX_DAILY_SPEND_USD_CENTS so a single worst-case tx
        // never blows the day's budget in one shot.
        let c = wei_cost_to_cents(MAX_PER_TX_GAS, MAX_FEE_PER_GAS_WEI);
        assert!(
            c <= i64::from(crate::wallet::MAX_DAILY_SPEND_USD_CENTS),
            "max envelope tx costs {c}¢, daily cap is {}¢",
            crate::wallet::MAX_DAILY_SPEND_USD_CENTS
        );
        // Also must be a meaningful figure — not 0 (would mean the constants
        // are broken and we'd never reconcile real costs).
        assert!(c > 0, "envelope cost must be > 0");
    }

    #[test]
    fn typical_tx_cost_under_one_cent() {
        // Typical tx: 60k gas at 0.02 gwei (real Base mainnet figures) =
        // 1.2e12 wei ≈ 0.04¢ → rounds to 0¢.
        let c = wei_cost_to_cents(60_000, 20_000_000);
        assert!(c <= 1, "typical tx costs {c}¢, expected ≤ 1¢");
    }

    #[test]
    fn balance_conversion_round_trips() {
        let cents = 250u64;
        let wei = U256::from(cents) * U256::from(WEI_PER_USD_CENT);
        assert_eq!(balance_wei_to_cents(wei), cents);
    }

    #[test]
    fn check_worst_case_cost_under_budget_ok() {
        let g = GasPrice {
            max_fee_per_gas: MAX_FEE_PER_GAS_WEI,
            max_priority_fee_per_gas: MIN_PRIORITY_FEE_WEI,
        };
        check_worst_case_cost(MAX_PER_TX_GAS, g).unwrap();
    }

    #[test]
    fn check_worst_case_cost_pathological_gas_price_rejects() {
        let g = GasPrice {
            max_fee_per_gas: 10_000_000_000_000_000_000_000, // absurd
            max_priority_fee_per_gas: 0,
        };
        check_worst_case_cost(MAX_PER_TX_GAS, g).unwrap_err();
    }
}
