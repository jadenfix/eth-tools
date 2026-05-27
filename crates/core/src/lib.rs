//! Core types for eth-tools: ERC-8004 chain registry, manifest schema, errors.

pub mod chains;
pub mod denied_reason;
pub mod errors;
pub mod events;
pub mod manifest;

pub use chains::{Chain, CHAINS};
pub use denied_reason::DeniedReason;
pub use errors::Error;

// Intentionally NOT re-exporting events at the crate root.
// `Transfer` collides with ERC-20 / ERC-721 / ERC-1155 / DEX events; flattening
// the ERC-8004 event names at the crate root would create a guaranteed naming
// conflict the first time a worker adds ERC-20 support. Callers must spell out
// `eth_tools_core::events::Registered` etc.
