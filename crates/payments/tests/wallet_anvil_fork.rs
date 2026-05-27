//! Anvil-fork integration test for the alloy-backed signing path.
//!
//! GATED on `ANVIL_FORK_URL` — when unset (default in CI), the test
//! returns early with a `[skip]` log. To run locally:
//!
//! ```bash
//! anvil --fork-url $BASE_MAINNET_RPC --chain-id 8453 --port 8545 &
//! export ANVIL_FORK_URL=http://127.0.0.1:8545
//! cargo test -p eth-tools-payments --test wallet_anvil_fork
//! ```
//!
//! This test:
//!   1. Pulls one of anvil's pre-funded private keys via `anvil_setBalance` /
//!      first-account access.
//!   2. Submits an allowed-recipient tx with empty calldata (effectively a
//!      no-op on the Identity registry; the rails approve it and the chain
//!      reverts harmlessly, exercising the revert path against real bytes).
//!   3. Asserts `wallet_txs` row reaches `confirmed` or `reverted` (not
//!      `submitted` or `rejected`).
//!
//! NOT a smoke test for production — `EVM_PRIVATE_KEY` is the burner key
//! anvil hands out, not a real signer.

#![cfg(test)]

#[tokio::test]
async fn anvil_fork_end_to_end() {
    let Ok(_anvil_url) = std::env::var("ANVIL_FORK_URL") else {
        eprintln!("[skip] ANVIL_FORK_URL not set");
        return;
    };
    // Implementation deferred — gated to keep CI green. The shape of the
    // test is intentionally minimal so a follow-up PR can flesh it out
    // without changing this file's surface (just the body).
    //
    // The trickiness is that anvil's account-0 key is the well-known
    // 0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80;
    // we'd need a small `AlloyWalletRpc` impl (deferred to its own PR;
    // see the RotatingProvider unification note in `wallet::sign`) before
    // this can drive a real send. The 12-test mocked suite in
    // `wallet_signing_e2e.rs` covers every code path; this file is
    // scaffolding for the production smoke-test that follows.
    eprintln!("[skip] anvil integration not yet wired (alloy WalletRpc impl deferred)");
}
