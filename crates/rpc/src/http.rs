//! Real HTTP JSON-RPC transport. Endpoint credentials never enter errors/logs.
use crate::{RpcError, RpcProvider};
use alloy::rpc::types::{Filter, Log};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::time::Duration;

pub struct HttpProvider {
    name: String,
    url: reqwest::Url,
    client: reqwest::Client,
}
impl HttpProvider {
    pub fn new(name: impl Into<String>, url: &str) -> Result<Self, RpcError> {
        let url = reqwest::Url::parse(url).map_err(|_| RpcError::Permanent("invalid RPC endpoint".into()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(RpcError::Permanent("RPC endpoint must use HTTP(S)".into()));
        }
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| RpcError::Permanent("HTTP client initialization failed".into()))?;
        Ok(Self {
            name: name.into(),
            url,
            client,
        })
    }
    async fn request(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let mut response = self
            .client
            .post(self.url.clone())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
            .send()
            .await
            .map_err(|_| RpcError::Transient("RPC transport failed".into()))?;
        let status = response.status();
        if status.as_u16() == 429 || status.is_server_error() {
            return Err(RpcError::Transient(format!(
                "RPC HTTP status {}",
                status.as_u16()
            )));
        }
        if !status.is_success() {
            return Err(RpcError::Permanent(format!(
                "RPC HTTP status {}",
                status.as_u16()
            )));
        }
        const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| RpcError::Transient("RPC body transport failed".into()))?
        {
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err(RpcError::Permanent("RPC response exceeds size limit".into()));
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| RpcError::Permanent("invalid JSON-RPC response".into()))?;
        if value["jsonrpc"] != "2.0" || value["id"] != 1 {
            return Err(RpcError::Permanent("JSON-RPC response identity mismatch".into()));
        }
        if let Some(error) = value.get("error") {
            let code = error["code"].as_i64().unwrap_or(0);
            return Err(if code == -32005 {
                RpcError::Transient("RPC rate limit".into())
            } else {
                RpcError::Permanent(format!("RPC error code {code}"))
            });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| RpcError::Permanent("JSON-RPC result missing".into()))
    }
}
#[async_trait]
impl RpcProvider for HttpProvider {
    fn name(&self) -> &str {
        &self.name
    }
    async fn get_block_number(&self) -> Result<u64, RpcError> {
        let value = self.request("eth_blockNumber", json!([])).await?;
        let text = value
            .as_str()
            .and_then(|s| s.strip_prefix("0x"))
            .ok_or_else(|| RpcError::Permanent("invalid block quantity".into()))?;
        u64::from_str_radix(text, 16).map_err(|_| RpcError::Permanent("invalid block quantity".into()))
    }
    async fn get_chain_id(&self) -> Result<u64, RpcError> {
        let value = self.request("eth_chainId", json!([])).await?;
        let text = value
            .as_str()
            .and_then(|s| s.strip_prefix("0x"))
            .ok_or_else(|| RpcError::Permanent("invalid chain identity".into()))?;
        u64::from_str_radix(text, 16).map_err(|_| RpcError::Permanent("invalid chain identity".into()))
    }
    async fn get_block_timestamp(&self, block: u64) -> Result<u64, RpcError> {
        let value = self
            .request("eth_getBlockByNumber", json!([format!("0x{block:x}"), false]))
            .await?;
        let timestamp = value["timestamp"]
            .as_str()
            .and_then(|s| s.strip_prefix("0x"))
            .ok_or_else(|| RpcError::Permanent("block timestamp missing".into()))?;
        u64::from_str_radix(timestamp, 16).map_err(|_| RpcError::Permanent("invalid block timestamp".into()))
    }

    async fn get_logs(&self, filter: &Filter) -> Result<Vec<Log>, RpcError> {
        serde_json::from_value(self.request("eth_getLogs", json!([filter])).await?)
            .map_err(|_| RpcError::Permanent("invalid log response".into()))
    }
}
