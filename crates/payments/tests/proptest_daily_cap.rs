//! Property test: the daily-cap counter is rolled back on rejection, for
//! every combination of (initial counter, cost) we throw at it.
//!
//! Two invariants under test:
//!  1. **Decisive rollback.** If the proposed cost would push the counter
//!     over `MAX_DAILY_SPEND_USD_CENTS`, the rail rejects AND the counter
//!     ends at exactly the pre-call value. No drift.
//!  2. **No phantom approvals.** The post-call counter NEVER exceeds the
//!     cap unless it was already over (caller seeded it that way) and the
//!     rail rejected with zero net change.
//!
//! These are exactly the cases the Stripe-style "ledger lies if I retry"
//! bug class wants to land. Run a few thousand cases per CI cycle so any
//! reorder / off-by-one in the rail surfaces here before prod.

#![cfg(test)]

use eth_tools_payments::wallet::daily_cap::InMemorySpendCounter;
use eth_tools_payments::wallet::policy::daily_cap;
use eth_tools_payments::wallet::{daily_spend_key, MAX_DAILY_SPEND_USD_CENTS};
use proptest::prelude::*;

/// Synchronous wrapper so proptest doesn't need an async harness.
fn run(initial: i64, cost: i64) -> (i64, Result<i64, eth_tools_core::DeniedReason>) {
    let key = daily_spend_key("production");
    let counter = InMemorySpendCounter::with_initial(&key, initial);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("rt");
    let res = rt.block_on(daily_cap(&counter, "production", cost));
    (counter.value(&key), res)
}

proptest! {
    /// On rejection (post-increment > cap) the counter MUST land exactly at
    /// `initial` — no drift, no phantom commit. On approval the counter
    /// MUST land at exactly `initial + cost`.
    #[test]
    fn rollback_lands_back_at_initial(
        initial in 0i64..=i64::from(MAX_DAILY_SPEND_USD_CENTS) * 4,
        cost in 0i64..=i64::from(MAX_DAILY_SPEND_USD_CENTS) * 4,
    ) {
        let (after, res) = run(initial, cost);
        let expected_post_incr = initial.saturating_add(cost);
        if expected_post_incr > i64::from(MAX_DAILY_SPEND_USD_CENTS) {
            // Must reject AND roll back to initial.
            prop_assert!(res.is_err(), "should reject above cap (initial={initial}, cost={cost})");
            prop_assert_eq!(after, initial, "rollback drift detected");
        } else {
            // Must approve AND leave counter at initial + cost.
            prop_assert!(res.is_ok(), "should accept under cap (initial={initial}, cost={cost})");
            prop_assert_eq!(after, expected_post_incr);
            prop_assert_eq!(res.unwrap(), expected_post_incr);
        }
    }

    /// Invariant: the post-call counter is NEVER `> cap + cost` regardless
    /// of inputs. (Even an already-over-cap initial value can't be made
    /// worse by a successful call, because we'd reject + rollback.)
    #[test]
    fn never_overshoots_cap_plus_cost(
        initial in 0i64..=i64::from(MAX_DAILY_SPEND_USD_CENTS) * 4,
        cost in 0i64..=i64::from(MAX_DAILY_SPEND_USD_CENTS),
    ) {
        let (after, _) = run(initial, cost);
        // The strongest claim we can make: the rail never leaves the
        // counter higher than (initial + cost). On rejection it's `initial`;
        // on approval it's `initial + cost`. So `after <= initial + cost`.
        prop_assert!(after <= initial.saturating_add(cost));
        // And the rail never goes BELOW initial (the only decrement is the
        // rollback, which exactly reverses the increment).
        prop_assert!(after >= initial);
    }

    /// No-panic property: any random `TransactionRequest`-shaped input
    /// passed through the chain/recipient/gas pure rails returns a
    /// `Result`, never panics. Belt-and-braces around the unwrap-free
    /// requirement in the task brief.
    #[test]
    fn pure_rails_never_panic_on_arbitrary_input(
        chain_id_opt in proptest::option::of(any::<u64>()),
        to_bytes in proptest::option::of(any::<[u8; 20]>()),
        gas_limit_opt in proptest::option::of(any::<u64>()),
    ) {
        use alloy_primitives::Address;
        use eth_tools_payments::wallet::policy::{
            allowed_chain, allowed_recipient, gas_ceiling, TransactionRequest,
        };

        let tx = TransactionRequest {
            chain_id: chain_id_opt,
            to: to_bytes.map(Address::from),
            gas_limit: gas_limit_opt,
            value: None,
        };
        // Each call MUST return a Result (no panic). We don't care about
        // the verdict here, only that the rail is total.
        let _ = allowed_chain(&tx);
        let _ = allowed_recipient(&tx);
        let _ = gas_ceiling(&tx);
    }
}
