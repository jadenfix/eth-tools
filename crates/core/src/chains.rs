//! ERC-8004 chain registry. Addresses are universal across deployments — see
//! https://github.com/erc-8004/erc-8004-contracts/blob/master/scripts/addresses.ts
//!
//! Bootstrap entries: Base mainnet (launch chain per plan §5) and Base Sepolia
//! (testnet for verification drills). Additional chains land in v1.x via the
//! multi-chain dispatcher.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chain {
    pub chain_id: u64,
    pub name: &'static str,
    pub identity_registry: [u8; 20],
    pub reputation_registry: [u8; 20],
    pub validation_registry: [u8; 20],
    pub is_testnet: bool,
    /// Block number at which the Identity registry was deployed; used by the
    /// scraper as its `from_block` floor on first run.
    pub genesis_block: u64,
}

// Universal ERC-8004 contract addresses (same across all 23 mainnets + 25 testnets).
const IDENTITY_REGISTRY: [u8; 20] = hex_lit("8004A169FB4a3325136EB29fA0ceB6D2e539a432");
const REPUTATION_REGISTRY: [u8; 20] = hex_lit("8004BAa17C55a88189AE136b182e5fdA19dE9b63");
const VALIDATION_REGISTRY: [u8; 20] = hex_lit("8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58");

pub const CHAINS: &[Chain] = &[
    Chain {
        chain_id: 8453,
        name: "base",
        identity_registry: IDENTITY_REGISTRY,
        reputation_registry: REPUTATION_REGISTRY,
        validation_registry: VALIDATION_REGISTRY,
        is_testnet: false,
        // Actual Base mainnet deploy block of IdentityRegistry per BaseScan
        // (contract creator tx of 0x8004A169…). Same block also holds the
        // Reputation/Validation registries — all three CREATE2-deployed in
        // a single tx.
        genesis_block: 41_663_783,
    },
    Chain {
        chain_id: 84532,
        name: "base-sepolia",
        identity_registry: IDENTITY_REGISTRY,
        reputation_registry: REPUTATION_REGISTRY,
        validation_registry: VALIDATION_REGISTRY,
        is_testnet: true,
        // TODO(phase-4): verify Base Sepolia deploy block via Etherscan v2
        // API once we have a key. 21_000_000 is a conservative pre-deploy
        // floor that keeps the scraper from sweeping years of empty blocks.
        genesis_block: 21_000_000,
    },
];

pub fn by_id(chain_id: u64) -> Option<&'static Chain> {
    CHAINS.iter().find(|c| c.chain_id == chain_id)
}

pub fn by_name(name: &str) -> Option<&'static Chain> {
    CHAINS.iter().find(|c| c.name.eq_ignore_ascii_case(name))
}

const fn hex_lit(s: &str) -> [u8; 20] {
    let bytes = s.as_bytes();
    assert!(bytes.len() == 40, "hex literal must be 40 chars (20 bytes)");
    let mut out = [0u8; 20];
    let mut i = 0;
    while i < 20 {
        out[i] = (hex_nibble(bytes[i * 2]) << 4) | hex_nibble(bytes[i * 2 + 1]);
        i += 1;
    }
    out
}

const fn hex_nibble(b: u8) -> u8 {
    match b {
        b'0'..=b'9' => b - b'0',
        b'a'..=b'f' => b - b'a' + 10,
        b'A'..=b'F' => b - b'A' + 10,
        _ => panic!("invalid hex digit"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_mainnet_present() {
        let c = by_id(8453).unwrap();
        assert_eq!(c.name, "base");
        assert!(!c.is_testnet);
    }

    #[test]
    fn base_sepolia_present() {
        let c = by_name("base-sepolia").unwrap();
        assert!(c.is_testnet);
    }

    #[test]
    fn universal_addresses_match() {
        // Every chain shares the same registry addresses.
        for c in CHAINS {
            assert_eq!(c.identity_registry, IDENTITY_REGISTRY);
            assert_eq!(c.reputation_registry, REPUTATION_REGISTRY);
            assert_eq!(c.validation_registry, VALIDATION_REGISTRY);
        }
    }

    #[test]
    fn hex_lit_roundtrip() {
        let addr = hex_lit("8004A169FB4a3325136EB29fA0ceB6D2e539a432");
        assert_eq!(addr[0], 0x80);
        assert_eq!(addr[1], 0x04);
        assert_eq!(addr[19], 0x32);
    }
}
