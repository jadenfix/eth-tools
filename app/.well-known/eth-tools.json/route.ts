// Self-describing kit metadata. Consumed by tooling that crawls trustless-agent
// runtimes. Static for now; will be signed once we have a kit signing key.
// Served via Cache-Control: public, max-age=300 (vercel.json).

export const dynamic = 'force-static';

export function GET() {
  return Response.json(
    {
      name: 'eth-tools',
      version: '0.0.1',
      description:
        'Self-maintaining runtime for ERC-8004 trustless agents. Agent-first: HTTP API · MCP server · CLI.',
      homepage: 'https://eth-tools.dev',
      docs: 'https://eth-tools.dev/docs',
      llms_txt: 'https://eth-tools.dev/llms.txt',
      openapi: 'https://eth-tools.dev/openapi.json',
      surfaces: {
        http: { base: 'https://eth-tools.dev/api/v1', auth: 'Bearer' },
        mcp: {
          endpoint: 'https://eth-tools.dev/api/mcp',
          transport: 'streamable-http',
          auth_discovery: 'https://eth-tools.dev/.well-known/oauth-protected-resource',
          tools_available: [
            'find_agent', 'inspect_agent', 'search_agents',
            'validate_manifest', 'hash_manifest',
            'read_feedback', 'read_validation', 'health',
            'register_agent', 'give_feedback', 'set_agent_uri', 'request_validation',
          ],
          tools_planned: [
            'generate_manifest', 'check_access', 'explain_access',
            'invoke', 'estimate_gas', 'revoke_feedback', 'set_agent_wallet',
            'mcp_from_agent_card', 'wallet_status', 'respond_validation',
          ],
        },
        cli: { package: 'eth-tools', registry: 'https://www.npmjs.com/package/eth-tools' },
        dashboard: 'https://eth-tools.dev/dashboard',
      },
      spec: 'ERC-8004',
      spec_url: 'https://eips.ethereum.org/EIPS/eip-8004',
      contracts: {
        identity:   '0x8004A169FB4a3325136EB29fA0ceB6D2e539a432',
        reputation: '0x8004BAa17C55a88189AE136b182e5fdA19dE9b63',
        validation: '0x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58',
      },
      chains_supported: ['base'],
      chains_planned: ['base-sepolia', 'optimism', 'arbitrum', 'mainnet'],
      payments: { protocol: 'x402', facilitator: 'coinbase-cdp', token: 'USDC' },
      repo: 'https://github.com/jadenfix/eth-tools',
      issues: 'https://github.com/jadenfix/eth-tools/issues',
      license: 'MIT',
    },
    {
      headers: { 'content-type': 'application/json; charset=utf-8' },
    },
  );
}
