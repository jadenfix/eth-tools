// RFC 9728 — OAuth Protected Resource metadata for the MCP server.
// Plan §8.3.

export const dynamic = 'force-static';

export function GET() {
  return Response.json({
    resource: 'https://eth-tools.dev/api/mcp',
    authorization_servers: ['https://eth-tools.dev'],
    bearer_methods_supported: ['header'],
    resource_documentation: 'https://eth-tools.dev/docs/auth',
  });
}
