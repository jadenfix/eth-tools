//! Core types for eth-tools: ERC-8004 chain registry, manifest schema, errors.

pub mod chains;
pub mod denied_reason;
pub mod errors;
pub mod manifest;

pub use chains::{Chain, CHAINS};
pub use denied_reason::DeniedReason;
pub use errors::Error;
