// Server component — receives the hand-curated agent list from
// `fixtures/well-known-agents.json` via props. Horizontal scroll is pure CSS
// (overflow-x-auto + snap), so no JS shipped for this section.

import Link from 'next/link';
import { safeExternalHref } from '@/lib/rust';

export type FeaturedAgent = {
  chain: string;
  agentId: string;
  label: string;
  description: string;
  manifestUrl: string;
  websiteUrl?: string | null;
};

export default function FeaturedAgents({ agents }: { agents: FeaturedAgent[] }) {
  if (agents.length === 0) {
    return null;
  }
  return (
    <section aria-labelledby="featured-heading" className="mx-auto max-w-6xl px-6 py-16">
      <div className="flex items-baseline justify-between">
        <h2 id="featured-heading" className="text-2xl font-semibold tracking-tight">
          Featured agents
        </h2>
        <Link
          href="/dashboard/agents"
          className="text-sm text-zinc-400 underline-offset-2 hover:text-zinc-200 hover:underline"
        >
          See all →
        </Link>
      </div>
      <ul
        role="list"
        data-testid="featured-agents"
        className="mt-6 flex snap-x snap-mandatory gap-4 overflow-x-auto pb-2"
      >
        {agents.map((a) => {
          const manifestHref = safeExternalHref(a.manifestUrl);
          const websiteHref = safeExternalHref(a.websiteUrl ?? null);
          return (
            <li
              key={`${a.chain}/${a.agentId}`}
              className="min-w-[280px] max-w-[320px] flex-shrink-0 snap-start rounded-xl border border-zinc-800 bg-zinc-900/40 p-5"
            >
              <div className="flex items-center justify-between text-xs">
                <span className="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-zinc-300">
                  {a.chain}
                </span>
                <span className="font-mono text-zinc-500">#{a.agentId}</span>
              </div>
              <h3 className="mt-3 text-base font-medium text-zinc-100">{a.label}</h3>
              <p className="mt-1 line-clamp-3 text-sm text-zinc-400">{a.description}</p>
              <div className="mt-4 flex gap-3 text-xs">
                {manifestHref ? (
                  <a
                    href={manifestHref}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-zinc-300 underline-offset-2 hover:underline"
                  >
                    manifest
                  </a>
                ) : null}
                {websiteHref ? (
                  <a
                    href={websiteHref}
                    target="_blank"
                    rel="noopener noreferrer"
                    className="text-zinc-300 underline-offset-2 hover:underline"
                  >
                    website
                  </a>
                ) : null}
              </div>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
