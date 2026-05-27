//! Core types for eth-tools: ERC-8004 chain registry, manifest schema, errors.

pub mod chains;
pub mod denied_reason;
pub mod errors;
pub mod events;
pub mod manifest;
pub mod safe_fetch;

pub use chains::{Chain, CHAINS};
pub use denied_reason::DeniedReason;
pub use errors::Error;
pub use events::{
    MetadataSet, Registered, Transfer, URIUpdated, METADATA_SET_TOPIC, REGISTERED_TOPIC, TRANSFER_TOPIC,
    URI_UPDATED_TOPIC,
};
