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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tool_count_matches_plan() {
        // Plan §9.2 lists 20 tools + wallet_status + health.
        assert_eq!(TOOL_NAMES.len(), 22);
    }
}
