# eth-tools

**The self-maintaining runtime for ERC-8004 trustless agents.**

`npm` for trustless agents — discoverable, validated, live, paid, and maintained by other agents.

Three machine surfaces (HTTP API · MCP server · CLI) and a human dashboard, all backed by autonomous Rust worker agents that continuously scrape the on-chain Identity / Reputation / Validation registries so the data plane stays fresh without human intervention.

## Status

Pre-alpha. Bootstrapping infrastructure. See [the plan](https://github.com/jadenfix/eth-tools/blob/main/AGENTS.md) for the roadmap.

## Quickstart (once shipped — pre-alpha today)

```bash
# Read
curl https://eth-tools.dev/api/v1/agents | jq

# CLI
npx eth-tools find "wallet risk"

# MCP (Claude / Cursor)
npx eth-tools mcp install
```

> The endpoints above 404 today. Pre-alpha. See [AGENTS.md](AGENTS.md) for the local-dev quickstart.

## License

MIT
