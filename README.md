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

## Native registry read path

The native branch implements real HTTP JSON-RPC providers (including bounded timeouts, rate-limit failover and block timestamps), a bounded Base identity-registry indexer, read API CLI commands, raw-byte Keccak manifest hashes, and three read-only MCP tools. Signed transactions/payment tools and the other seven worker bodies remain unimplemented and are not advertised as executable MCP tools.

```bash
# Existing local PostgreSQL with migrations applied by dev-server:
export DATABASE_URL=postgres://dev:dev@localhost:5432/eth_tools
cargo run -p eth-tools-dev-server
cargo run -p eth-tools-cli -- inspect base/42
cargo run -p eth-tools-cli -- find 42
cargo run -p eth-tools-cli -- manifest hash agent-card.json
# Index at most 2,000 confirmed Base blocks. Configure explicit endpoints;
# the indexer never signs transactions and dryrun rolls back rows/cursor.
RPC_URL_PRIMARY=http://127.0.0.1:8545 cargo run -p eth-tools-cli -- index-registry --dryrun
```

`find` filters the returned indexed page and preserves `next_cursor`; it is not a full-text search across all historical agents. `manifest validate` checks the current AgentCard structure and HTTP(S) endpoints, not a complete ERC-8004 schema. The local `/api/mcp` route supports initialize, tools/list and calls for health, search_agents and inspect_agent. Hosted MCP requires explicit `ETH_TOOLS_API_URL`.

Run `cargo test -p eth-tools-rpc` for real loopback HTTP transport/failover tests. The dedicated `native registry E2E` CI gate requires PostgreSQL and runs `cargo test -p eth-tools-workers --test registry_live -- --ignored` plus `python3 scripts/native-read-e2e.py`: real RPC events → atomic agent/cursor writes → rollback/replay/reconnect, then actual native server → CLI/MCP → restart persistence. Missing storage fails this gate rather than silently skipping. The indexer currently tracks Registered, URIUpdated and Transfer on Base; metadata/history/reputation/validation and chain-reorg rollback remain future work.

The bootstrap Registered and URIUpdated event signatures were corrected against the upstream [ERC-8004 specification](https://github.com/erc-8004/erc-8004-contracts/blob/master/ERC8004SPEC.md): Registered uses (uint256,string,address), and URIUpdated includes the indexed updatedBy address. Old incompatible event topics are rejected rather than used for live indexing.
