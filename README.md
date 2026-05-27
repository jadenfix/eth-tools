# eth-tools

The self-maintaining runtime for ERC-8004 trustless agents — discoverable, validated, live, paid, and maintained by other agents.

## Quickstart

Three ways to use eth-tools today:

```bash
# 1. Read the registry over HTTP
curl https://eth-tools.dev/api/v1/agents | jq

# 2. CLI (published with the cli phase)
npx -y eth-tools find "wallet risk"
```

**3. Add to Claude / Cursor** — paste into your MCP config
(`~/Library/Application Support/Claude/claude_desktop_config.json` on macOS):

```json
{
  "mcpServers": {
    "eth-tools": {
      "command": "npx",
      "args": ["-y", "eth-tools", "mcp"],
      "env": { "ETH_TOOLS_API_KEY": "<key from /dashboard/keys>" }
    }
  }
}
```

## What it is

- **Registry indexer** — Rust workers tail the ERC-8004 Identity, Reputation, and Validation registries on each supported chain and write into Postgres.
- **MCP server** — 20 tools (read agents, validate manifests, query reputation, submit signed writes) over the Model Context Protocol.
- **Paid-write relay** — optional env-var wallet for clients without keys, capped per-transaction with five hard rails (recipient allowlist, chain allowlist, balance ceiling, daily cap, kill switch).

## What it is not

- Not a new payment standard. Writes go out as standard EVM transactions over x402.
- Not a UI fork of an existing registry explorer. The dashboard is thin and read-only; the data plane is the product.
- Not a TEE / confidential-compute platform. The wallet runs in plain Vercel functions behind the hard rails above.

## Architecture

```
                ┌──────────────────────────────────────────┐
   Chains  ──▶  │  crates/workers  ──▶  Postgres (Neon)     │
   (Base,       │  (indexer · manifest fetcher · prober)    │
   …)           └─────────────┬────────────────────────────┘
                              │
              ┌───────────────┼─────────────────────────────┐
              ▼               ▼                             ▼
        crates/api       crates/mcp                    crates/cli
        (HTTP)           (MCP server)                  (npx)
              │               │                             │
              └───────┬───────┴─────────────┬───────────────┘
                      ▼                     ▼
               eth-tools.dev          your-laptop / your-agent
               (Next.js dash)
```

## Links

- Dashboard: <https://eth-tools.dev/dashboard>
- Docs: <https://eth-tools.dev/docs>
- Status: <https://eth-tools.dev/dashboard/workers>
- Issues: <https://github.com/jadenfix/eth-tools/issues>

## License & contributing

MIT. See [`AGENTS.md`](AGENTS.md) for the contributor / agent guide, including the security non-negotiables that apply to any change in `crates/payments/` or `api/cron/wallet_*.rs`.
