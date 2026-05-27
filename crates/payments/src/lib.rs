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

pub mod denials;
pub mod edge_config;
pub mod wallet;

pub use wallet::{
    ALLOWED_CHAIN_ID, ALLOWED_RECIPIENTS, ALLOWED_RECIPIENTS_HEX, KILL_SWITCH_KEY, MAX_BALANCE_USD_CENTS,
    MAX_DAILY_SPEND_USD_CENTS, MAX_PER_TX_GAS, PRIVATE_KEY_ENV, PUBLIC_KEY_ENV,
};

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

    #[test]
    fn allowed_recipients_match_hex_constants() {
        // Both the strongly-typed Address constants and the legacy hex string
        // constants must agree on the registry list. A drift here would let a
        // future contributor "add a registry" in one place but not the other,
        // silently widening the allowlist.
        assert_eq!(ALLOWED_RECIPIENTS.len(), ALLOWED_RECIPIENTS_HEX.len());
        for (addr, hex) in ALLOWED_RECIPIENTS.iter().zip(ALLOWED_RECIPIENTS_HEX) {
            let formatted = format!("{:x}", addr).to_lowercase();
            assert_eq!(formatted, hex.to_lowercase());
        }
    }
}
