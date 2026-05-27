import Link from 'next/link';
import { listAgents, type Agent } from '@/lib/rust';

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
    <main className="mx-auto max-w-6xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">Agents</h1>
        <p className="text-xs text-zinc-500">
          source: <span className="font-mono">{list.source}</span> · staleness:{' '}
          <span className="font-mono">{list.staleness_ms} ms</span>
        </p>
      </div>

      <div className="overflow-x-auto rounded-lg border border-zinc-800">
        <table className="min-w-full divide-y divide-zinc-800 text-sm">
          <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
            <tr>
              <th className="px-4 py-2">chain</th>
              <th className="px-4 py-2">agent_id</th>
              <th className="px-4 py-2">owner</th>
              <th className="px-4 py-2">updated_at</th>
              <th className="px-4 py-2">has_wallet</th>
              <th className="px-4 py-2">agent_uri</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-zinc-800/80">
            {rows.length === 0 ? (
              <tr>
                <td className="px-4 py-6 text-center text-zinc-500" colSpan={6}>
                  No agents indexed yet.
                </td>
              </tr>
            ) : (
              rows.map((agent) => (
                <tr
                  key={`${agent.chain}/${agent.agent_id}`}
                  className="hover:bg-zinc-900/40"
                >
                  <td className="px-4 py-2">
                    <Link
                      href={`/dashboard/agents/${encodeURIComponent(agent.chain)}/${encodeURIComponent(agent.agent_id)}`}
                      className="font-mono underline-offset-2 hover:underline"
                    >
                      {agent.chain}
                    </Link>
                  </td>
                  <td className="px-4 py-2 font-mono">{agent.agent_id}</td>
                  <td className="px-4 py-2 font-mono">{shortHex(agent.owner)}</td>
                  <td className="px-4 py-2 text-zinc-400">{agent.updated_at}</td>
                  <td className="px-4 py-2">{agent.agent_wallet ? 'yes' : 'no'}</td>
                  <td className="px-4 py-2">
                    {agent.agent_uri ? (
                      <a
                        href={agent.agent_uri}
                        target="_blank"
                        rel="noreferrer"
                        className="underline underline-offset-2"
                      >
                        link
                      </a>
                    ) : (
                      <span className="text-zinc-600">—</span>
                    )}
                  </td>
                </tr>
              ))
            )}
          </tbody>
        </table>
      </div>

      <div className="mt-6 flex items-center justify-between text-sm">
        <Link href="/dashboard" className="text-zinc-400 underline-offset-2 hover:underline">
          ← Back to dashboard
        </Link>
        {list.next_cursor ? (
          <Link
            href={`/dashboard/agents?cursor=${encodeURIComponent(list.next_cursor)}`}
            className="rounded-md border border-zinc-700 px-3 py-1.5 hover:bg-zinc-900"
          >
            Load more →
          </Link>
        ) : (
          <span className="text-zinc-500">end of list</span>
        )}
      </div>
    </main>
  );
}
