// /llms.txt — machine-readable summary for AI agents discovering this service.
// Spec: https://llmstxt.org. Served with public, max-age=300 cache (vercel.json).

export const dynamic = 'force-static';

export function GET() {
  const body = `# eth-tools

> Self-maintaining runtime for ERC-8004 trustless agents. Three machine
> surfaces (HTTP API · MCP server · CLI) plus a thin terminal-style
> dashboard for humans. Eight autonomous Rust worker agents keep the
> on-chain data plane fresh — without human intervention. Agent-first.

## Quickstart

- HTTP:   curl -s https://eth-tools.dev/api/v1/agents | jq
- CLI:    npx -y eth-tools find "wallet risk"
- MCP:    add the server at https://eth-tools.dev/api/mcp to Claude or Cursor
          (paste-ready JSON at https://eth-tools.dev — "Add to Claude" button).

## HTTP API surface

- GET    https://eth-tools.dev/api/v1/health           # liveness + cursor lag per chain
- GET    https://eth-tools.dev/api/v1/agents           # paginated list (keyset cursor)
- GET    https://eth-tools.dev/api/v1/agents/{chain}/{id}
- POST   https://eth-tools.dev/api/v1/agents/search    # body: { query, limit, filters }
- POST   https://eth-tools.dev/api/v1/manifest/validate
- POST   https://eth-tools.dev/api/v1/manifest/hash
- POST   https://eth-tools.dev/api/v1/manifest/generate
- POST   https://eth-tools.dev/api/v1/access/check
- POST   https://eth-tools.dev/api/v1/access/explain
- POST   https://eth-tools.dev/api/v1/invoke/prepare   # returns calldata, no signing
- POST   https://eth-tools.dev/api/v1/invoke           # x402-gated; signs + sends
- POST   https://eth-tools.dev/api/v1/reputation/give
- GET    https://eth-tools.dev/api/v1/reputation/{chain}/{id}
- POST   https://eth-tools.dev/api/v1/validation/request
- POST   https://eth-tools.dev/api/v1/validation/respond
- GET    https://eth-tools.dev/api/v1/validation/{chain}/{hash}

Typed contract: https://eth-tools.dev/openapi.json (Cache-Control: public, max-age=60).
Envelope: { data, staleness_ms, source } on success; DeniedReason { code, policy_version,
evaluator, expected, got, override_hint } on rejection. Every POST honors Idempotency-Key.

## MCP

- Endpoint: https://eth-tools.dev/api/mcp  (Streamable HTTP transport)
- Auth:     Bearer API key, advertised via https://eth-tools.dev/.well-known/oauth-protected-resource
            (RFC 9728). Same key works for HTTP + MCP.
- Tools available now (12):
    find_agent · inspect_agent · search_agents
    validate_manifest · hash_manifest
    read_feedback · read_validation · health
    register_agent · give_feedback · set_agent_uri · request_validation
- Tools planned (not yet shipped — agents calling these get JSON-RPC method-not-found):
    generate_manifest · check_access · explain_access
    invoke · estimate_gas · revoke_feedback · set_agent_wallet
    mcp_from_agent_card · wallet_status · respond_validation

## CLI

- Install:  npm i -g eth-tools  (or use npx -y eth-tools)
- Auth:     eth-tools auth login          # opens dashboard, polls for issued key
- Surfaces: find · inspect · manifest {validate|hash|generate} · register · invoke · watch
            · mcp install · mcp from-card · workers status · wallet status · backfill

## Discovery

- llms.txt:           https://eth-tools.dev/llms.txt
- Kit metadata:       https://eth-tools.dev/.well-known/eth-tools.json
- OAuth resource:     https://eth-tools.dev/.well-known/oauth-protected-resource
- OpenAPI:            https://eth-tools.dev/openapi.json
- Docs:               https://eth-tools.dev/docs
- Dashboard:          https://eth-tools.dev/dashboard (humans · terminal-style)
- Status:             https://eth-tools.dev/dashboard/workers
- Source:             https://github.com/jadenfix/eth-tools

## Chains

- Supported now:      base (8453)
- Planned:            base-sepolia · optimism · arbitrum · mainnet
- ERC-8004 addresses (same on all chains):
    Identity     0x8004A169FB4a3325136EB29fA0ceB6D2e539a432
    Reputation   0x8004BAa17C55a88189AE136b182e5fdA19dE9b63
    Validation   0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58

## Status

Pre-alpha. See https://github.com/jadenfix/eth-tools/issues for shipping and
https://eth-tools.dev/dashboard/workers for live runtime telemetry.

## License

MIT.
`;
  return new Response(body, {
    headers: { 'content-type': 'text/plain; charset=utf-8' },
  });
}
