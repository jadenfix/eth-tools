// /llms.txt — machine-readable summary for AI agents discovering this service.
// Spec: https://llmstxt.org

export const dynamic = 'force-static';

export function GET() {
  const body = `# eth-tools

> Self-maintaining runtime for ERC-8004 trustless agents. Three machine surfaces (HTTP API · MCP server · CLI) and a Next.js dashboard. Eight autonomous Rust worker agents keep the on-chain data plane fresh.

## API

- Read: GET https://eth-tools.dev/api/v1/agents
- Detail: GET https://eth-tools.dev/api/v1/agents/{chain}/{id}
- Health: GET https://eth-tools.dev/api/v1/health

## MCP

- Endpoint: https://eth-tools.dev/api/mcp
- Auth: Bearer API key (advertised via /.well-known/oauth-protected-resource)

## CLI

\`npx eth-tools find "wallet risk"\`

## Status

Bootstrap. See https://github.com/jadenfix/eth-tools for the roadmap.
`;
  return new Response(body, { headers: { 'content-type': 'text/plain; charset=utf-8' } });
}
