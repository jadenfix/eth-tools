'use client';

// Try-it-in-30-seconds section. Three tabs:
//   curl   — read-side smoke against the public REST API.
//   npx    — the published CLI (lands with the cli phase).
//   claude — opens a modal with paste-ready MCP JSON.
//
// The MCP JSON is the same shape Claude Desktop / Code reads from
// `~/Library/Application Support/Claude/claude_desktop_config.json` (macOS)
// or `~/.config/Claude/claude_desktop_config.json` (Linux). We render the
// JSON verbatim so users can copy-paste without templating.

import { useEffect, useState } from 'react';

type Tab = 'curl' | 'npx' | 'claude';

const SNIPPETS: Record<Exclude<Tab, 'claude'>, string> = {
  curl: 'curl https://eth-tools.dev/api/v1/agents | jq',
  npx: 'npx -y eth-tools find "wallet risk"',
};

const MCP_CONFIG = `{
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

  const openClaude = () => {
    setTab('claude');
    setModalOpen(true);
  };

  const copyMcp = async () => {
    try {
      await navigator.clipboard.writeText(MCP_CONFIG);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // Clipboard API can fail in non-secure contexts; user falls back to
      // manual select-all + copy. No need to surface an error.
    }
  };

  return (
    <section aria-labelledby="try-it-heading" className="mx-auto max-w-3xl px-6 py-16">
      <h2 id="try-it-heading" className="text-2xl font-semibold tracking-tight">
        Try it in 30 seconds
      </h2>
      <div
        role="tablist"
        aria-label="Try it"
        className="mt-6 inline-flex rounded-lg border border-zinc-800 bg-zinc-900/40 p-1 text-sm"
      >
        <TabButton current={tab} value="curl" onSelect={setTab}>curl</TabButton>
        <TabButton current={tab} value="npx" onSelect={setTab}>npx eth-tools</TabButton>
        <TabButton current={tab} value="claude" onSelect={openClaude}>Add to Claude</TabButton>
      </div>

      <div className="mt-4">
        {tab === 'claude' ? (
          <div
            role="tabpanel"
            aria-labelledby="tab-claude"
            data-testid="tab-panel-claude"
            className="rounded-lg border border-zinc-800 bg-zinc-950 p-4 text-sm text-zinc-400"
          >
            Click <strong className="text-zinc-200">Add to Claude</strong> above to open the
            MCP config snippet.{' '}
            <button
              type="button"
              onClick={openClaude}
              className="underline underline-offset-2 hover:text-zinc-200"
            >
              Reopen the modal
            </button>
            .
          </div>
        ) : (
          <pre
            role="tabpanel"
            aria-labelledby={`tab-${tab}`}
            data-testid={`tab-panel-${tab}`}
            className="overflow-x-auto rounded-lg border border-zinc-800 bg-zinc-950 px-4 py-3 font-mono text-sm text-zinc-200"
          >
            <code>{SNIPPETS[tab]}</code>
          </pre>
        )}
      </div>

      {modalOpen ? (
        <div
          role="dialog"
          aria-modal="true"
          aria-labelledby="mcp-modal-title"
          data-testid="mcp-modal"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 px-4"
          onClick={() => setModalOpen(false)}
        >
          <div
            className="w-full max-w-xl rounded-xl border border-zinc-800 bg-zinc-950 p-6 shadow-2xl"
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-start justify-between gap-4">
              <div>
                <h3 id="mcp-modal-title" className="text-lg font-semibold">
                  Add eth-tools to Claude
                </h3>
                <p className="mt-1 text-sm text-zinc-400">
                  Paste this into your Claude Desktop MCP config (
                  <code className="rounded bg-zinc-900 px-1 py-0.5 text-xs">
                    ~/Library/Application Support/Claude/claude_desktop_config.json
                  </code>
                  ) and restart Claude.
                </p>
              </div>
              <button
                type="button"
                aria-label="Close"
                onClick={() => setModalOpen(false)}
                className="rounded-md p-1 text-zinc-400 hover:bg-zinc-900 hover:text-zinc-100"
              >
                <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
                  <path d="M18 6 6 18M6 6l12 12" />
                </svg>
              </button>
            </div>
            <pre className="mt-4 max-h-80 overflow-auto rounded-lg border border-zinc-800 bg-black px-4 py-3 font-mono text-xs text-zinc-200">
              <code>{MCP_CONFIG}</code>
            </pre>
            <div className="mt-4 flex items-center justify-end gap-2">
              <button
                type="button"
                onClick={copyMcp}
                className="rounded-md border border-zinc-700 bg-zinc-900 px-3 py-1.5 text-sm hover:bg-zinc-800"
              >
                {copied ? 'Copied' : 'Copy JSON'}
              </button>
              <button
                type="button"
                onClick={() => setModalOpen(false)}
                className="rounded-md bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white"
              >
                Done
              </button>
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
      className={
        'rounded-md px-3 py-1.5 transition-colors ' +
        (active ? 'bg-zinc-100 text-zinc-900' : 'text-zinc-300 hover:bg-zinc-800/60')
      }
    >
      {children}
    </button>
  );
}
