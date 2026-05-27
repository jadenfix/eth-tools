import Link from 'next/link';
import { notFound } from 'next/navigation';
import { ApiError, getAgent, safeExternalHref, type Agent } from '@/lib/rust';

type RouteParams = Promise<{ chain: string; agent_id: string }>;

function Row({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="grid grid-cols-[180px_1fr] gap-4 border-b border-zinc-800/60 py-2 text-sm">
      <dt className="text-zinc-500">{label}</dt>
      <dd className="break-all font-mono text-zinc-100">{children}</dd>
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
    <main className="mx-auto max-w-3xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">
          {agent.chain}/{agent.agent_id}
        </h1>
        <Link
          href="/dashboard/agents"
          className="text-sm text-zinc-400 underline-offset-2 hover:underline"
        >
          ← Back to list
        </Link>
      </div>

      <dl className="rounded-lg border border-zinc-800 px-4 py-2">
        <Row label="chain">{agent.chain}</Row>
        <Row label="chain_id">{agent.chain_id}</Row>
        <Row label="agent_id">{agent.agent_id}</Row>
        <Row label="owner">{agent.owner}</Row>
        <Row label="agent_wallet">
          {agent.agent_wallet ?? <span className="text-zinc-500">null</span>}
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
                  className="underline underline-offset-2"
                >
                  {agent.agent_uri}
                </a>
              );
            }
            if (agent.agent_uri) {
              // URI present but scheme rejected — show as plain text so the
              // operator still sees what was recorded on-chain.
              return <span className="text-amber-400">{agent.agent_uri}</span>;
            }
            return <span className="text-zinc-500">null</span>;
          })()}
        </Row>
        <Row label="registered_at">{agent.registered_at}</Row>
        <Row label="updated_at">{agent.updated_at}</Row>
      </dl>
    </main>
  );
}
