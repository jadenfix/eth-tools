import Link from 'next/link';
import { notFound } from 'next/navigation';
import { ApiError, getAgent, safeExternalHref, type Agent } from '@/lib/rust';

type RouteParams = Promise<{ chain: string; agent_id: string }>;

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div
      className="grid grid-cols-[140px_1fr] gap-4 py-2 text-sm"
      style={{ borderBottom: '1px dashed var(--term-border)' }}
    >
      <dt className="text-xs uppercase tracking-[0.1em]" style={{ color: 'var(--term-muted)' }}>
        {label}
      </dt>
      <dd className="break-all" style={{ color: 'var(--term-fg)' }}>
        {children}
      </dd>
    </div>
  );
}

export default async function AgentDetailPage({
  params,
}: {
  params: RouteParams;
}) {
  const { chain, agent_id } = await params;

  let agent: Agent;
  try {
    const detail = await getAgent(chain, agent_id);
    agent = detail.data;
  } catch (err) {
    if (err instanceof ApiError && err.status === 404) {
      notFound();
    }
    throw err;
  }

  return (
    <main className="relative z-10 mx-auto max-w-3xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <Link href="/dashboard/agents" className="term-link">agents</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>
          {agent.chain}/{agent.agent_id}
        </span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/agents/{agent.chain}/{agent.agent_id}</span>
        </div>
        <div className="term-body">
          <p className="text-sm">
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              eth-tools inspect {agent.chain}/{agent.agent_id}
            </span>
          </p>

          <dl className="mt-4">
            <Row label="chain">{agent.chain}</Row>
            <Row label="chain_id">{agent.chain_id}</Row>
            <Row label="agent_id">
              <span style={{ color: 'var(--term-accent-2)' }}>#{agent.agent_id}</span>
            </Row>
            <Row label="owner">{agent.owner}</Row>
            <Row label="agent_wallet">
              {agent.agent_wallet ?? (
                <span style={{ color: 'var(--term-muted)' }}>null</span>
              )}
            </Row>
            <Row label="agent_uri">
              {(() => {
                const href = safeExternalHref(agent.agent_uri);
                if (href) {
                  return (
                    <a
                      href={href}
                      target="_blank"
                      rel="noopener noreferrer"
                      className="term-link"
                    >
                      {agent.agent_uri}
                    </a>
                  );
                }
                if (agent.agent_uri) {
                  return (
                    <span style={{ color: 'var(--term-warn)' }}>
                      {agent.agent_uri}{' '}
                      <span className="term-pill term-pill-warn">scheme-rejected</span>
                    </span>
                  );
                }
                return <span style={{ color: 'var(--term-muted)' }}>null</span>;
              })()}
            </Row>
            <Row label="registered_at">{agent.registered_at}</Row>
            <Row label="updated_at">{agent.updated_at}</Row>
          </dl>

          <div className="mt-5 flex items-center justify-between text-sm">
            <Link href="/dashboard/agents" className="term-link">
              ← back to list
            </Link>
            <a
              href={`/api/v1/agents/${encodeURIComponent(agent.chain)}/${encodeURIComponent(agent.agent_id)}`}
              className="term-link text-xs"
            >
              raw json →
            </a>
          </div>
        </div>
      </section>
    </main>
  );
}
