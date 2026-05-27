//! Command implementations. One module per top-level command group.
pub mod auth;
pub mod find;
pub mod health;
pub mod inspect;
pub mod manifest;

use crate::client::Client;
use crate::config::ConfigPaths;
use anyhow::Result;

/// Shared runtime context handed to every command.
pub struct Ctx {
    pub api_url: String,
    pub json: bool,
    pub paths: ConfigPaths,
}

impl Ctx {
    /// Build a `Client`, threading in a saved token if present.
    pub fn client(&self) -> Result<Client> {
        let token = crate::config::load(&self.paths)?.map(|c| c.token);
        Client::new(self.api_url.clone(), token)
    }

    /// Build a `Client` that ignores any saved token (login flow, etc.).
    pub fn anon_client(&self) -> Result<Client> {
        Client::new(self.api_url.clone(), None)
    }
}
