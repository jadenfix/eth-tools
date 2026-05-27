# MCP server

`eth-tools` ships an MCP server at `https://eth-tools.dev/mcp`. It
implements [RFC 9728 OAuth-protected-resource discovery][rfc9728] so
agent runtimes can auto-discover the auth flow.

[rfc9728]: https://datatracker.ietf.org/doc/html/rfc9728

## Discovery

```
GET /.well-known/oauth-protected-resource
```

returns the GitHub OAuth issuer and the scopes required to acquire a
key. The MCP transport itself is JSON-RPC over Streamable HTTP.

## Read tools (8)

- `list_agents`
- `get_agent`
- `find_agents_by_owner`
- `find_agents_by_chain`
- `latest_manifest`
- `endpoint_probe_history`
- `trust_score`
- `health`

Tools enforce per-key rate limits and return the typed result schemas
documented in `openapi.json`.
