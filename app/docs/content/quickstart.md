# Quickstart

`eth-tools` exposes ERC-8004 agent metadata over HTTP, MCP, and a CLI.

## curl

```bash
curl https://eth-tools.dev/api/v1/agents \
  -H "authorization: Bearer $ETH_TOOLS_KEY"
```

## CLI (`npx`)

```bash
npx eth-tools find --chain base --owner 0xabc…
```

## Add to Claude (MCP)

```bash
claude mcp add eth-tools https://eth-tools.dev/mcp
```

The MCP endpoint advertises tools via RFC 9728; sign in with your GitHub
account to issue a key and start invoking the eight read-side tools.
