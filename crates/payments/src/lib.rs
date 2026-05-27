//! Wallet rails (plan §10.2) — the hard constants that bound worst-case loss.
//!
//! These constants are defined here, in code, on purpose. Putting them in
//! config or env would let a single compromised env var widen the blast radius.
//! Changing them requires a code review + signed commit.

// Plan §10.5 #1: any accidental `println!`/`dbg!`/`eprintln!` inside the wallet
// crate is a hard build error. The signed-tx path runs in production with the
// `EVM_PRIVATE_KEY` env var loaded; the only place a secret can leak to stdout
// is a careless debug print. Hoist these to deny so a future contributor cannot
// land one. The corresponding clippy guidance lives in `clippy.toml`.
#![deny(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)]

pub mod wallet {
    /// Base mainnet only at MVP. Plan §10.5 control #3.
    pub const ALLOWED_CHAIN_ID: u64 = 8453;

    /// The three ERC-8004 registries are the only addresses we will sign
    /// transactions to. Plan §10.5 control #2.
    pub const ALLOWED_RECIPIENTS_HEX: &[&str] = &[
        "8004A169FB4a3325136EB29fA0ceB6D2e539a432", // Identity
        "8004BAa17C55a88189AE136b182e5fdA19dE9b63", // Reputation
        "8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58", // Validation
    ];

    /// $5 hard ceiling on the hot-wallet balance. W8 sweeps above this. §10.5 #4.
    pub const MAX_BALANCE_USD_CENTS: u32 = 500;

    /// $1/day spend cap, enforced by Upstash atomic INCRBY. §10.5 #5.
    pub const MAX_DAILY_SPEND_USD_CENTS: u32 = 100;

    /// Per-tx gas cap — refuse > 500k. Plan §10.2 rail #4.
    pub const MAX_PER_TX_GAS: u64 = 500_000;

    /// Vercel Edge Config key that gates every signed transaction. §10.5 #6.
    pub const KILL_SWITCH_KEY: &str = "wallet_enabled";

    /// Public env var name (never the private key). Logged at boot for sanity.
    pub const PUBLIC_KEY_ENV: &str = "EVM_PUBLIC_KEY";

    /// Private key env var. **NEVER LOG.** Sensitive in Vercel.
    pub const PRIVATE_KEY_ENV: &str = "EVM_PRIVATE_KEY";
}

// Compile-time invariants — promoted from runtime tests so weakening the
// constants fails the build, not just the test suite.
const _BALANCE_GE_4X_DAILY: () = {
    assert!(wallet::MAX_BALANCE_USD_CENTS >= wallet::MAX_DAILY_SPEND_USD_CENTS * 4);
};
const _BASE_MAINNET_ONLY: () = {
    assert!(wallet::ALLOWED_CHAIN_ID == 8453);
};
const _GAS_CAP_REASONABLE: () = {
    // Above 1M gas, a single tx can cost dollars on Base spikes.
    assert!(wallet::MAX_PER_TX_GAS <= 1_000_000);
};
const _RECIPIENTS_ARE_REGISTRIES: () = {
    assert!(wallet::ALLOWED_RECIPIENTS_HEX.len() == 3);
    // Length check at compile time; full hex validation in the test below.
    let mut i = 0;
    while i < wallet::ALLOWED_RECIPIENTS_HEX.len() {
        assert!(wallet::ALLOWED_RECIPIENTS_HEX[i].len() == 40);
        i += 1;
    }
};

#[cfg(test)]
mod tests {
    use super::wallet::*;

    #[test]
    fn three_recipients_one_per_registry() {
        assert_eq!(ALLOWED_RECIPIENTS_HEX.len(), 3);
        for hex in ALLOWED_RECIPIENTS_HEX {
            assert_eq!(hex.len(), 40, "{hex} is not 20 bytes");
            assert!(
                hex.chars().all(|c| c.is_ascii_hexdigit()),
                "{hex} is not pure hex"
            );
        }
    }

    #[test]
    fn private_key_env_name_is_sensitive() {
        // Catches a careless rename that would orphan the Vercel Sensitive flag.
        assert_eq!(PRIVATE_KEY_ENV, "EVM_PRIVATE_KEY");
    }
}
