// Server component — receives the hand-curated agent list from
// `fixtures/well-known-agents.json` via props. Rendered as an `ls -la`
// style listing inside a terminal window, with each agent on its own
// row plus inline manifest / website links.

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
    <section aria-labelledby="featured-heading" className="mt-12 term-window">
      <div className="term-titlebar">
        <span className="term-dot term-dot-r" aria-hidden="true" />
        <span className="term-dot term-dot-y" aria-hidden="true" />
        <span className="term-dot term-dot-g" aria-hidden="true" />
        <span className="ml-3">ls -la /agents/featured</span>
      </div>
      <div className="term-body">
        <div className="flex items-baseline justify-between">
          <p className="text-sm">
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>ls -la /agents/featured</span>
          </p>
          <Link
            href="/dashboard/agents"
            className="term-link text-xs"
          >
            see-all →
          </Link>
        </div>
        <h2 id="featured-heading" className="sr-only">
          Featured agents
        </h2>
        <p className="mt-2 text-xs" style={{ color: 'var(--term-muted)' }}>
          # total {agents.length} · hand-curated · replaced before public launch
        </p>

        <ul
          role="list"
          data-testid="featured-agents"
          className="mt-3 divide-y text-sm"
          style={{ borderColor: 'var(--term-border)' }}
        >
          {agents.map((a) => {
            const manifestHref = safeExternalHref(a.manifestUrl);
            const websiteHref = safeExternalHref(a.websiteUrl ?? null);
            return (
              <li
                key={`${a.chain}/${a.agentId}`}
                className="py-3"
                style={{ borderColor: 'var(--term-border)' }}
              >
                <div className="flex flex-wrap items-center gap-x-3 gap-y-1">
                  <span className="term-pill term-pill-acc">{a.chain}</span>
                  <span style={{ color: 'var(--term-muted)' }}>#</span>
                  <span style={{ color: 'var(--term-accent-2)' }}>{a.agentId}</span>
                  <span style={{ color: 'var(--term-fg)' }}>{a.label}</span>
                  <span className="ml-auto flex gap-3 text-xs">
                    {manifestHref ? (
                      <a
                        href={manifestHref}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="term-link"
                      >
                        manifest
                      </a>
                    ) : null}
                    {websiteHref ? (
                      <a
                        href={websiteHref}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="term-link"
                      >
                        website
                      </a>
                    ) : null}
                  </span>
                </div>
                <p
                  className="mt-1 line-clamp-2 text-xs"
                  style={{ color: 'var(--term-fg-dim)' }}
                >
                  {a.description}
                </p>
              </li>
            );
          })}
        </ul>
      </div>
    </section>
  );
}
