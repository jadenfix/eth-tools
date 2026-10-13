//! MCP server tool definitions. Bootstrap stub — `rmcp` integration lands in
//! the next PR.

pub const TOOL_NAMES: &[&str] = &[
    "find_agent",
    "inspect_agent",
    "search_agents",
    "validate_manifest",
    "hash_manifest",
    "generate_manifest",
    "check_access",
    "explain_access",
    "invoke",
    "estimate_gas",
    "give_feedback",
    "read_feedback",
    "revoke_feedback",
    "request_validation",
    "respond_validation",
    "read_validation",
    "register_agent",
    "set_agent_uri",
    "set_agent_wallet",
    "mcp_from_agent_card",
    "wallet_status",
    "health",
];

/// Read-only JSON-RPC MCP surface backed by the same HTTP API as the CLI.
/// Mutating/payment tools are deliberately not advertised until implemented.
pub async fn handle(request: serde_json::Value, api_base: &str) -> serde_json::Value {
    use serde_json::json;
    let id = request.get("id").cloned().unwrap_or(serde_json::Value::Null);
    if request["jsonrpc"] != "2.0" || !request.is_object() {
        return json!({"jsonrpc":"2.0","id":id,"error":{"code":-32600,"message":"Invalid JSON-RPC request"}});
    }
    let result = match request["method"].as_str() {
        Some("initialize") => Ok(
            json!({"protocolVersion":"2025-03-26","capabilities":{"tools":{}},"serverInfo":{"name":"eth-tools","version":env!("CARGO_PKG_VERSION")}}),
        ),
        Some("ping") => Ok(json!({})),
        Some("tools/list") => Ok(json!({"tools":[
            {"name":"health","description":"Read actual registry API health","inputSchema":{"type":"object","properties":{},"additionalProperties":false}},
            {"name":"inspect_agent","description":"Inspect an indexed agent","inputSchema":{"type":"object","properties":{"chain":{"type":"string"},"agent_id":{"type":"string"}},"required":["chain","agent_id"],"additionalProperties":false}},
            {"name":"search_agents","description":"Read one indexed page with its continuation cursor","inputSchema":{"type":"object","properties":{},"additionalProperties":false}}
        ]})),
        Some("tools/call") => {
            let name = request["params"]["name"].as_str().unwrap_or("");
            let args = &request["params"]["arguments"];
            let path = match name {
                "health" => Some(vec!["api", "v1", "health"]),
                "search_agents" => Some(vec!["api", "v1", "agents"]),
                "inspect_agent" => match (args["chain"].as_str(), args["agent_id"].as_str()) {
                    (Some(chain), Some(agent))
                        if !chain.is_empty()
                            && !agent.is_empty()
                            && agent.bytes().all(|c| c.is_ascii_digit()) =>
                    {
                        Some(vec!["api", "v1", "agents", chain, agent])
                    }
                    _ => None,
                },
                _ => None,
            };
            match path {
                None => Err((-32602, "Unknown tool or invalid arguments")),
                Some(segments) => match read_api(api_base, &segments).await {
                    Ok(value) => {
                        Ok(json!({"content":[{"type":"text","text":value.to_string()}],"isError":false}))
                    }
                    Err(_) => Ok(
                        json!({"content":[{"type":"text","text":"Registry API request failed"}],"isError":true}),
                    ),
                },
            }
        }
        _ => Err((-32601, "Method not found")),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
        Err((code, message)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}}),
    }
}
async fn read_api(base: &str, segments: &[&str]) -> Result<serde_json::Value, ()> {
    let mut url = reqwest::Url::parse(base).map_err(|_| ())?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(());
    }
    url.path_segments_mut().map_err(|_| ())?.clear().extend(segments);
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| ())?
        .get(url)
        .send()
        .await
        .map_err(|_| ())?
        .error_for_status()
        .map_err(|_| ())?
        .json()
        .await
        .map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_count_matches_plan() {
        // Plan §9.2 lists 20 tools + wallet_status + health.
        assert_eq!(TOOL_NAMES.len(), 22);
    }
}
