import Link from 'next/link';
import { listAgents, safeExternalHref, type Agent } from '@/lib/rust';

type SearchParams = Promise<{ cursor?: string | string[] }>;

function shortHex(addr: string | null | undefined): string {
  if (!addr) return '—';
  if (addr.length <= 12) return addr;
  return `${addr.slice(0, 6)}…${addr.slice(-4)}`;
}

function pickCursor(value: string | string[] | undefined): string | undefined {
  if (Array.isArray(value)) return value[0];
  return value;
}

export default async function AgentsListPage({
  searchParams,
}: {
  searchParams: SearchParams;
}) {
  const sp = await searchParams;
  const cursor = pickCursor(sp.cursor);

  const list = await listAgents({ limit: 50, cursor });
  const rows: Agent[] = list.data;

  return (
    <main className="relative z-10 mx-auto max-w-6xl px-4 py-10 sm:px-6">
      <p className="mb-4 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <Link href="/dashboard" className="term-link">dashboard</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>agents</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard/agents</span>
          <span className="ml-auto text-[10px]" style={{ color: 'var(--term-muted)' }}>
            source:<span style={{ color: 'var(--term-accent-2)' }}>{list.source}</span>{' '}
            · staleness:<span style={{ color: 'var(--term-accent-2)' }}>{list.staleness_ms}ms</span>
          </span>
        </div>
        <div className="term-body">
          <p className="text-sm">
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>
              eth-tools agents list --limit 50{cursor ? ` --cursor ${cursor.slice(0, 12)}…` : ''}
            </span>
          </p>

          <div className="mt-4 overflow-x-auto">
            <table
              className="min-w-full text-sm"
              style={{ borderCollapse: 'collapse' }}
            >
              <thead>
                <tr
                  className="text-left text-xs uppercase tracking-[0.1em]"
                  style={{ color: 'var(--term-muted)' }}
                >
                  <th className="px-3 py-2">chain</th>
                  <th className="px-3 py-2">agent_id</th>
                  <th className="px-3 py-2">owner</th>
                  <th className="px-3 py-2">updated_at</th>
                  <th className="px-3 py-2">wallet</th>
                  <th className="px-3 py-2">agent_uri</th>
                </tr>
              </thead>
              <tbody>
                {rows.length === 0 ? (
                  <tr>
                    <td
                      colSpan={6}
                      className="px-3 py-6 text-center"
                      style={{ color: 'var(--term-muted)' }}
                    >
                      # no agents indexed yet — workers haven&apos;t caught up
                    </td>
                  </tr>
                ) : (
                  rows.map((agent) => (
                    <tr
                      key={`${agent.chain}/${agent.agent_id}`}
                      style={{ borderTop: '1px dashed var(--term-border)' }}
                    >
                      <td className="px-3 py-2">
                        <Link
                          href={`/dashboard/agents/${encodeURIComponent(agent.chain)}/${encodeURIComponent(agent.agent_id)}`}
                          className="term-link"
                        >
                          {agent.chain}
                        </Link>
                      </td>
                      <td className="px-3 py-2" style={{ color: 'var(--term-accent-2)' }}>
                        #{agent.agent_id}
                      </td>
                      <td className="px-3 py-2" style={{ color: 'var(--term-fg-dim)' }}>
                        {shortHex(agent.owner)}
                      </td>
                      <td className="px-3 py-2" style={{ color: 'var(--term-muted)' }}>
                        {agent.updated_at}
                      </td>
                      <td className="px-3 py-2">
                        {agent.agent_wallet ? (
                          <span className="term-pill term-pill-ok">yes</span>
                        ) : (
                          <span className="term-pill">no</span>
                        )}
                      </td>
                      <td className="px-3 py-2">
                        {(() => {
                          const href = safeExternalHref(agent.agent_uri);
                          return href ? (
                            <a
                              href={href}
                              target="_blank"
                              rel="noopener noreferrer"
                              className="term-link"
                            >
                              link
                            </a>
                          ) : (
                            <span style={{ color: 'var(--term-muted)' }}>—</span>
                          );
                        })()}
                      </td>
                    </tr>
                  ))
                )}
              </tbody>
            </table>
          </div>

          <div className="mt-5 flex items-center justify-between text-sm">
            <Link href="/dashboard" className="term-link">
              ← back to dashboard
            </Link>
            {list.next_cursor ? (
              <Link
                href={`/dashboard/agents?cursor=${encodeURIComponent(list.next_cursor)}`}
                className="rounded-md px-3 py-1.5"
                style={{
                  border: '1px solid var(--term-border-2)',
                  color: 'var(--term-accent-2)',
                }}
              >
                next page →
              </Link>
            ) : (
              <span style={{ color: 'var(--term-muted)' }}>
                # end of list
              </span>
            )}
          </div>
        </div>
      </section>
    </main>
  );
}
