//! Rotating RPC provider (Alchemy primary, QuickNode fallback, public last resort).
//!
//! Implementation lands in the next PR. See plan §14.1.

/// Alchemy's `eth_getLogs` hard limit is 2_000 blocks per call. Other providers
/// are more permissive but we pin to the strictest so we never have to special-case.
pub const MAX_LOGS_PER_CALL: u64 = 2_000;

/// Process blocks up to `head - REORG_SAFETY_BLOCKS`; refetch the tail next tick.
pub const REORG_SAFETY_BLOCKS: u64 = 12;

/// Effective head for a worker given the current chain head.
pub fn safe_head(head_block: u64) -> u64 {
    head_block.saturating_sub(REORG_SAFETY_BLOCKS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_head_subtracts_reorg_window() {
        assert_eq!(safe_head(1_000_000), 999_988);
    }

    #[test]
    fn safe_head_saturates_at_zero() {
        assert_eq!(safe_head(5), 0);
    }
}
