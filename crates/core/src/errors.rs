//! Crate-wide error type.

use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("manifest: {0}")]
    Manifest(String),

    #[error("chain not registered: {0}")]
    UnknownChain(String),

    #[error("serde: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("denied: {0}")]
    Denied(String),
}
