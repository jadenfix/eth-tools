'use client';

// Try-it terminal — four tabs: curl, npx, claude, cursor.
// MCP-config snippets are rendered verbatim so users copy-paste cleanly.

import { useEffect, useState } from 'react';

type Tab = 'curl' | 'npx' | 'claude' | 'cursor';

const SNIPPETS: Record<Exclude<Tab, 'claude' | 'cursor'>, string> = {
  curl: 'curl -s https://eth-tools.dev/api/v1/agents | jq',
  npx: 'npx -y eth-tools find "wallet risk"',
};

const CLAUDE_CONFIG = `{
  "mcpServers": {
    "eth-tools": {
      "command": "npx",
      "args": ["-y", "eth-tools", "mcp"],
      "env": {
        "ETH_TOOLS_API_KEY": "<paste your key from /dashboard/keys>"
      }
    }
  }
}`;

const CURSOR_CONFIG = `{
  "mcpServers": {
    "eth-tools": {
      "url": "https://eth-tools.dev/api/mcp",
      "headers": {
        "Authorization": "Bearer <paste your key from /dashboard/keys>"
      }
    }
  }
}`;

export default function TryIt() {
  const [tab, setTab] = useState<Tab>('curl');
  const [modalOpen, setModalOpen] = useState(false);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    if (!modalOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setModalOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [modalOpen]);

  const openModal = (target: 'claude' | 'cursor') => {
    setTab(target);
    setModalOpen(true);
  };

  const modalConfig = tab === 'cursor' ? CURSOR_CONFIG : CLAUDE_CONFIG;
  const modalTarget = tab === 'cursor' ? 'Cursor' : 'Claude';
  const modalPath =
    tab === 'cursor'
      ? '~/.cursor/mcp.json (or per-project .cursor/mcp.json)'
      : '~/Library/Application Support/Claude/claude_desktop_config.json';

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(modalConfig);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API can fail in non-secure contexts; user select-copies manually.
    }
  };

  return (
    <section aria-labelledby="try-it-heading" className="mt-12 term-window">
      <div className="term-titlebar">
        <span className="term-dot term-dot-r" aria-hidden="true" />
        <span className="term-dot term-dot-y" aria-hidden="true" />
        <span className="term-dot term-dot-g" aria-hidden="true" />
        <span className="ml-3">try-it — 30 seconds — pick a surface</span>
      </div>
      <div className="term-body">
        <h2 id="try-it-heading" className="sr-only">
          Try it in 30 seconds
        </h2>

        <div
          role="tablist"
          aria-label="Try it"
          className="flex flex-wrap gap-1 border-b pb-2 text-xs"
          style={{ borderColor: 'var(--term-border)' }}
        >
          <TabButton current={tab} value="curl" onSelect={setTab}>
            $ curl
          </TabButton>
          <TabButton current={tab} value="npx" onSelect={setTab}>
            $ npx eth-tools
          </TabButton>
          <TabButton current={tab} value="claude" onSelect={() => openModal('claude')}>
            ▸ add to claude
          </TabButton>
          <TabButton current={tab} value="cursor" onSelect={() => openModal('cursor')}>
            ▸ add to cursor
          </TabButton>
        </div>

        <div className="mt-4">
          {tab === 'claude' || tab === 'cursor' ? (
            <div
              role="tabpanel"
              aria-labelledby={`tab-${tab}`}
              data-testid={`tab-panel-${tab}`}
              className="text-sm"
              style={{ color: 'var(--term-fg-dim)' }}
            >
              <p>
                <span className="term-prompt-bare">$</span>{' '}
                <span style={{ color: 'var(--term-fg)' }}>
                  open mcp.json — paste config
                </span>
              </p>
              <p className="mt-2">
                Click <strong style={{ color: 'var(--term-accent-2)' }}>▸ add to {tab}</strong>{' '}
                above to open the MCP config snippet.{' '}
                <button
                  type="button"
                  onClick={() => openModal(tab)}
                  className="term-link"
                >
                  Reopen the modal
                </button>
                .
              </p>
            </div>
          ) : (
            <pre
              role="tabpanel"
              aria-labelledby={`tab-${tab}`}
              data-testid={`tab-panel-${tab}`}
              className="overflow-x-auto rounded-md px-4 py-3 text-sm"
              style={{
                background: 'var(--term-bg)',
                border: '1px solid var(--term-border)',
                color: 'var(--term-fg)',
              }}
            >
              <code>
                <span className="term-prompt-bare">$</span> {SNIPPETS[tab]}
              </code>
            </pre>
          )}
        </div>

        <p className="mt-3 text-xs" style={{ color: 'var(--term-muted)' }}>
          # Anonymous reads are rate-limited; sign in at{' '}
          <a href="/dashboard/keys" className="term-link">
            /dashboard/keys
          </a>{' '}
          to issue an API key (write surfaces require one).
        </p>
      </div>

      {modalOpen ? (
        <div
          role="dialog"
          aria-modal="true"
          aria-labelledby="mcp-modal-title"
          data-testid="mcp-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/80 px-4"
          onClick={() => setModalOpen(false)}
        >
          <div
            className="term-window w-full max-w-xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="term-titlebar">
              <span className="term-dot term-dot-r" aria-hidden="true" />
              <span className="term-dot term-dot-y" aria-hidden="true" />
              <span className="term-dot term-dot-g" aria-hidden="true" />
              <span className="ml-3">add eth-tools to {modalTarget}</span>
              <button
                type="button"
                aria-label="Close"
                onClick={() => setModalOpen(false)}
                className="ml-auto rounded p-1"
                style={{ color: 'var(--term-muted)' }}
              >
                ✕
              </button>
            </div>
            <div className="term-body">
              <h3 id="mcp-modal-title" className="sr-only">
                Add eth-tools to {modalTarget}
              </h3>
              <p className="text-xs" style={{ color: 'var(--term-fg-dim)' }}>
                <span className="term-prompt-bare">$</span> $EDITOR{' '}
                <code style={{ color: 'var(--term-accent-2)' }}>{modalPath}</code>
              </p>
              <pre
                className="mt-3 max-h-80 overflow-auto rounded-md px-4 py-3 text-xs"
                style={{
                  background: 'var(--term-bg)',
                  border: '1px solid var(--term-border)',
                  color: 'var(--term-fg)',
                }}
              >
                <code>{modalConfig}</code>
              </pre>
              <div className="mt-4 flex items-center justify-end gap-2 text-sm">
                <button
                  type="button"
                  onClick={copy}
                  className="rounded-md px-3 py-1.5"
                  style={{
                    border: '1px solid var(--term-border-2)',
                    color: 'var(--term-accent-2)',
                  }}
                >
                  {copied ? '✓ copied' : 'copy json'}
                </button>
                <button
                  type="button"
                  onClick={() => setModalOpen(false)}
                  className="rounded-md px-3 py-1.5"
                  style={{
                    background: 'var(--term-prompt)',
                    color: 'var(--term-bg)',
                    fontWeight: 600,
                  }}
                >
                  done
                </button>
              </div>
            </div>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function TabButton({
  current,
  value,
  onSelect,
  children,
}: {
  current: Tab;
  value: Tab;
  onSelect: (t: Tab) => void;
  children: React.ReactNode;
}) {
  const active = current === value;
  return (
    <button
      type="button"
      id={`tab-${value}`}
      role="tab"
      aria-selected={active}
      onClick={() => onSelect(value)}
      className="rounded-t-md px-3 py-1.5 transition-colors"
      style={{
        background: active ? 'var(--term-bg)' : 'transparent',
        color: active ? 'var(--term-accent-2)' : 'var(--term-fg-dim)',
        borderTop: active ? '1px solid var(--term-border-2)' : '1px solid transparent',
        borderLeft: active ? '1px solid var(--term-border-2)' : '1px solid transparent',
        borderRight: active ? '1px solid var(--term-border-2)' : '1px solid transparent',
      }}
    >
      {children}
    </button>
  );
}
