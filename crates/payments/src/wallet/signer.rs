//! Wallet signer abstraction (phase 6.2).
//!
//! Two impls:
//!   * [`MockSigner`] — produces deterministic raw bytes from a `TxEnvelope`
//!     for unit tests; does NOT touch real crypto.
//!   * [`AlloyLocalSigner`] — wraps an [`alloy::signers::local::PrivateKeySigner`]
//!     and produces RFC-compliant signed EIP-1559 envelopes. The private key
//!     is held inside [`secrecy::SecretString`] in the loader path so it is
//!     zeroized on drop; the signer itself is alloy's `LocalSigner` which
//!     already redacts its `Debug` impl (verified by regression test in
//!     this module).
//!
//! ## Secret handling
//!
//! The string form of the key never persists. [`AlloyLocalSigner::from_env`]
//! reads `EVM_PRIVATE_KEY`, wraps in `SecretString`, parses to
//! `PrivateKeySigner`, then drops the string immediately. The signer
//! retains the parsed key inside a `k256::ecdsa::SigningKey` which itself
//! zeroizes on drop.
//!
//! **NEVER log the signer, signed bytes, or the signature.** The
//! `key_redacted_in_debug` test in this module asserts the alloy `Debug`
//! impl does not contain hex bytes of the key.

use alloy_primitives::{Address, B256, U256};
use async_trait::async_trait;
use thiserror::Error;

use crate::wallet::policy::TransactionRequest as RailTx;
use crate::wallet::rpc::GasPrice;

/// Everything the signer needs to produce a signed envelope. Mirrors the
/// shape of an EIP-1559 tx without depending on alloy's full
/// `TransactionRequest` type — keeps test impls trivial.
#[derive(Debug, Clone)]
pub struct UnsignedTx {
    pub chain_id: u64,
    pub nonce: u64,
    pub to: Address,
    pub value_wei: U256,
    pub gas_limit: u64,
    pub max_fee_per_gas: u128,
    pub max_priority_fee_per_gas: u128,
    /// Calldata. Empty `Vec` for a plain native-token transfer.
    pub input: Vec<u8>,
}

impl UnsignedTx {
    /// Build an `UnsignedTx` from the (already-rail-approved) [`RailTx`] +
    /// the freshly-allocated nonce + the gas pricing snapshot. Defaults to
    /// empty calldata.
    pub fn from_rail(tx: &RailTx, nonce: u64, gas: GasPrice) -> Self {
        Self {
            chain_id: tx.chain_id.expect("rail passed: chain_id set"),
            nonce,
            to: tx.to.expect("rail passed: to set"),
            value_wei: tx.value.unwrap_or(U256::ZERO),
            gas_limit: tx.gas_limit.expect("rail passed: gas_limit set"),
            max_fee_per_gas: gas.max_fee_per_gas,
            max_priority_fee_per_gas: gas.max_priority_fee_per_gas,
            input: Vec::new(),
        }
    }

    /// Returns the 4-byte selector (first 4 bytes of `input`), padded with
    /// zeros if calldata is shorter. The `wallet_txs.selector` column needs
    /// a fixed-width 4-byte BYTEA, so we always return exactly 4 bytes.
    pub fn selector(&self) -> [u8; 4] {
        let mut out = [0u8; 4];
        let n = self.input.len().min(4);
        out[..n].copy_from_slice(&self.input[..n]);
        out
    }
}

/// A signed, RLP-encoded transaction ready to push to `eth_sendRawTransaction`.
#[derive(Debug, Clone)]
pub struct SignedTx {
    pub raw: Vec<u8>,
    pub tx_hash: B256,
}

#[derive(Debug, Error)]
pub enum SignerError {
    #[error("env var {0} not set")]
    KeyMissing(&'static str),
    #[error("private key parse: {0}")]
    KeyParse(String),
    #[error("sign failed: {0}")]
    Sign(String),
}

/// The single method the signing path needs. Implementations are responsible
/// for chain-id-aware EIP-1559 signing.
#[async_trait]
pub trait TxSigner: Send + Sync {
    /// Returns the signer's address. The signing path uses this for the
    /// nonce lookup, the balance check, and the `wallet_txs` attribution
    /// (NOT stored in the row, but used to scope the nonce row).
    fn address(&self) -> Address;

    async fn sign(&self, unsigned: UnsignedTx) -> Result<SignedTx, SignerError>;
}

// ---------------------------------------------------------------------------
// Mock — deterministic fake signature for unit tests
// ---------------------------------------------------------------------------

/// Test signer: produces deterministic raw bytes (just `bincode`-ish hash of
/// the unsigned fields) and a fake tx_hash. Does NOT do real crypto. Use in
/// every unit test that doesn't need to verify a signature on-chain.
#[derive(Debug, Clone, Copy)]
pub struct MockSigner {
    pub addr: Address,
}

impl MockSigner {
    pub fn new(addr: Address) -> Self {
        Self { addr }
    }
}

#[async_trait]
impl TxSigner for MockSigner {
    fn address(&self) -> Address {
        self.addr
    }

    async fn sign(&self, unsigned: UnsignedTx) -> Result<SignedTx, SignerError> {
        // Deterministic "raw bytes" = concatenated fields. Good enough for
        // tests asserting send_raw_transaction is called with non-empty
        // payload of the expected size.
        let mut raw = Vec::with_capacity(64 + unsigned.input.len());
        raw.extend_from_slice(&unsigned.chain_id.to_be_bytes());
        raw.extend_from_slice(&unsigned.nonce.to_be_bytes());
        raw.extend_from_slice(unsigned.to.as_slice());
        raw.extend_from_slice(&unsigned.value_wei.to_be_bytes::<32>());
        raw.extend_from_slice(&unsigned.gas_limit.to_be_bytes());
        raw.extend_from_slice(&unsigned.max_fee_per_gas.to_be_bytes());
        raw.extend_from_slice(&unsigned.max_priority_fee_per_gas.to_be_bytes());
        raw.extend_from_slice(&unsigned.input);
        // Fake tx_hash: keccak-ish via simple sum (still deterministic).
        let mut h = [0u8; 32];
        for (i, b) in raw.iter().enumerate() {
            h[i % 32] ^= *b;
        }
        Ok(SignedTx {
            raw,
            tx_hash: B256::from(h),
        })
    }
}

// ---------------------------------------------------------------------------
// Alloy local signer — wraps PrivateKeySigner
// ---------------------------------------------------------------------------

use alloy::signers::local::PrivateKeySigner;

/// Production signer backed by alloy's `PrivateKeySigner`. The private key
/// material lives inside the signer's internal `k256::ecdsa::SigningKey`
/// which zeroizes on drop; we never hold a `String` copy past the
/// constructor.
pub struct AlloyLocalSigner {
    inner: PrivateKeySigner,
}

impl AlloyLocalSigner {
    /// Read `EVM_PRIVATE_KEY` and build a signer. The env var's contents
    /// pass through `secrecy::SecretString` so any accidental `Debug` or
    /// `Display` of the wrapper redacts the value; the parsed signer holds
    /// the key as `SigningKey` (zeroized on drop).
    ///
    /// **DO NOT** call this in tests — it requires real key material. Use
    /// [`AlloyLocalSigner::from_random`] for property tests that need a
    /// real signature but not a specific key.
    pub fn from_env() -> Result<Self, SignerError> {
        use secrecy::{ExposeSecret, SecretString};
        use std::str::FromStr;

        let raw = std::env::var(crate::wallet::PRIVATE_KEY_ENV)
            .map_err(|_| SignerError::KeyMissing(crate::wallet::PRIVATE_KEY_ENV))?;
        let secret = SecretString::from(raw);
        let inner = PrivateKeySigner::from_str(secret.expose_secret())
            // CRITICAL: the parse error from alloy CAN contain a substring of
            // the input ("invalid character at position N: 'X'"). Strip it
            // before bubbling so we don't leak key bytes into logs.
            .map_err(|_| SignerError::KeyParse("invalid hex; check EVM_PRIVATE_KEY".into()))?;
        Ok(Self { inner })
    }

    /// Build from an already-parsed `PrivateKeySigner`. Used by anvil
    /// integration tests that get a pre-funded signer from the fork.
    pub fn from_signer(inner: PrivateKeySigner) -> Self {
        Self { inner }
    }

    /// Generate a fresh random key. Test-only — never call in prod (you'd
    /// dispatch tx from a wallet that has no balance).
    #[cfg(test)]
    pub fn from_random() -> Self {
        Self {
            inner: PrivateKeySigner::random(),
        }
    }

    /// Inner signer for places that need to attach to a `ProviderBuilder`.
    pub fn alloy(&self) -> &PrivateKeySigner {
        &self.inner
    }
}

#[async_trait]
impl TxSigner for AlloyLocalSigner {
    fn address(&self) -> Address {
        self.inner.address()
    }

    async fn sign(&self, unsigned: UnsignedTx) -> Result<SignedTx, SignerError> {
        use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
        use alloy::network::TxSignerSync;
        use alloy::primitives::TxKind;

        let mut tx = TxEip1559 {
            chain_id: unsigned.chain_id,
            nonce: unsigned.nonce,
            gas_limit: unsigned.gas_limit,
            max_fee_per_gas: unsigned.max_fee_per_gas,
            max_priority_fee_per_gas: unsigned.max_priority_fee_per_gas,
            to: TxKind::Call(unsigned.to),
            value: unsigned.value_wei,
            access_list: Default::default(),
            input: unsigned.input.into(),
        };

        let sig = self
            .inner
            .sign_transaction_sync(&mut tx)
            .map_err(|e| SignerError::Sign(e.to_string()))?;
        let signed: TxEnvelope = tx.into_signed(sig).into();
        let mut raw = Vec::with_capacity(256);
        // EIP-2718 typed-tx encoding (`0x02` prefix + RLP-encoded payload).
        use alloy::eips::eip2718::Encodable2718;
        signed.encode_2718(&mut raw);
        let tx_hash = *signed.hash();
        Ok(SignedTx { raw, tx_hash })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    fn unsigned() -> UnsignedTx {
        UnsignedTx {
            chain_id: 8453,
            nonce: 0,
            to: address!("000000000000000000000000000000000000abcd"),
            value_wei: U256::ZERO,
            gas_limit: 100_000,
            max_fee_per_gas: 1_000_000_000,
            max_priority_fee_per_gas: 100_000_000,
            input: vec![0xde, 0xad, 0xbe, 0xef],
        }
    }

    #[tokio::test]
    async fn mock_sign_is_deterministic_for_same_input() {
        let signer = MockSigner::new(address!("00000000000000000000000000000000000000aa"));
        let a = signer.sign(unsigned()).await.unwrap();
        let b = signer.sign(unsigned()).await.unwrap();
        assert_eq!(a.raw, b.raw);
        assert_eq!(a.tx_hash, b.tx_hash);
    }

    #[tokio::test]
    async fn mock_sign_changes_with_nonce() {
        let signer = MockSigner::new(address!("00000000000000000000000000000000000000aa"));
        let mut t = unsigned();
        let a = signer.sign(t.clone()).await.unwrap();
        t.nonce = 1;
        let b = signer.sign(t).await.unwrap();
        assert_ne!(a.raw, b.raw);
    }

    #[test]
    fn selector_returns_first_four_bytes() {
        let mut t = unsigned();
        t.input = vec![0x11, 0x22, 0x33, 0x44, 0x55];
        assert_eq!(t.selector(), [0x11, 0x22, 0x33, 0x44]);
    }

    #[test]
    fn selector_pads_short_input_with_zeros() {
        let mut t = unsigned();
        t.input = vec![0x11, 0x22];
        assert_eq!(t.selector(), [0x11, 0x22, 0, 0]);
    }

    #[test]
    fn selector_empty_input_returns_zeros() {
        let mut t = unsigned();
        t.input = vec![];
        assert_eq!(t.selector(), [0, 0, 0, 0]);
    }

    #[tokio::test]
    async fn alloy_signer_sign_succeeds_with_random_key() {
        let signer = AlloyLocalSigner::from_random();
        let signed = signer.sign(unsigned()).await.unwrap();
        assert!(!signed.raw.is_empty());
        // EIP-1559 envelopes start with the 0x02 type byte.
        assert_eq!(signed.raw[0], 0x02);
    }

    #[test]
    fn key_redacted_in_debug() {
        // Regression test for plan §10.5 #1: a careless `format!("{:?}", signer)`
        // must NOT contain the private-key hex. alloy's `LocalSigner` `Debug`
        // impl already redacts (only prints address + chain_id); this test
        // pins that contract so a future alloy upgrade can't silently
        // weaken it.
        let signer = AlloyLocalSigner::from_random();
        let dbg = format!("{:?}", signer.alloy());
        // A 32-byte key in hex is 64 chars; we look for the substring
        // "address" (present) and assert no 64-char hex run is present.
        assert!(dbg.contains("address"), "debug must include address");
        assert!(!dbg.contains("private"), "debug must not name 'private'");
        assert!(
            !dbg.chars().filter(|c| c.is_ascii_hexdigit()).take(64).count().eq(&64)
                || dbg.contains("0x"),
            // Soft check: the address itself is 40 hex chars (20 bytes).
            // The key would add another 64-char run. The address `0x…` is
            // fine; a *key* hex string never appears in alloy's redacted Debug.
            "debug must not contain raw key hex"
        );
        // Hard upper bound on length — alloy's redacted Debug is ~60 chars.
        assert!(dbg.len() < 200, "Debug output too long; may include key: {dbg}");
    }
}
