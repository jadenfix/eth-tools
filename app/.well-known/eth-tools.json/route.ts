// Self-describing kit metadata. Consumed by tooling that crawls trustless-agent
// runtimes. Will be signed once we have a kit signing key.

export const dynamic = 'force-static';

export function GET() {
  return Response.json({
    name: 'eth-tools',
    version: '0.0.1',
    surfaces: {
      http: '/api/v1',
      mcp: '/api/mcp',
      cli: 'npm:eth-tools',
    },
    chains_supported: ['base'],
    chains_planned: ['base-sepolia', 'optimism', 'arbitrum', 'mainnet'],
    spec: 'ERC-8004',
    docs: '/docs',
    repo: 'https://github.com/jadenfix/eth-tools',
  });
}
