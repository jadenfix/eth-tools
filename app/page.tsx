export default function Page() {
  return (
    <main style={{ fontFamily: 'system-ui, sans-serif', padding: 40, maxWidth: 760, lineHeight: 1.5 }}>
      <h1 style={{ fontSize: 32, margin: 0 }}>eth-tools</h1>
      <p style={{ fontSize: 18, color: '#555' }}>
        npm for trustless agents — discoverable, validated, live, paid, and maintained by other agents.
      </p>
      <hr style={{ margin: '24px 0' }} />
      <p>
        <strong>Status:</strong> bootstrap. The infrastructure is up; functionality lands phase by phase.
      </p>
      <pre style={{ background: '#f6f6f6', padding: 16, borderRadius: 8 }}>
{`curl https://eth-tools.dev/api/v1/health | jq`}
      </pre>
      <p>
        <a href="https://github.com/jadenfix/eth-tools">GitHub</a> · {' '}
        <a href="/dashboard">Dashboard</a> · {' '}
        <a href="/docs">Docs</a>
      </p>
    </main>
  );
}
