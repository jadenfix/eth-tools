'use client';

// Client-side keys manager. Renders the list, the create-modal, and the
// "your new key" reveal panel. All mutation goes through the Next.js Route
// Handlers in `app/api/v1/keys/*`, which Auth-check then forward to Rust.
//
// The plaintext is rendered exactly once, with a one-click copy button and
// a loud warning. We deliberately keep it in component state (not in
// localStorage / sessionStorage) so a navigation away clears it.

import { useCallback, useState } from 'react';
import type { IssuedKey, KeyView } from '@/lib/keys';

const ALL_SCOPES = ['read', 'write', 'wallet'] as const;
type Scope = (typeof ALL_SCOPES)[number];

export function KeysClient({ initial }: { initial: KeyView[] }) {
  const [keys, setKeys] = useState<KeyView[]>(initial);
  const [showCreate, setShowCreate] = useState(false);
  const [revealing, setRevealing] = useState<IssuedKey | null>(null);
  const [revoking, setRevoking] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    const res = await fetch('/api/v1/keys', { cache: 'no-store' });
    if (res.ok) {
      const body = (await res.json()) as { data: KeyView[] };
      setKeys(body.data);
    }
  }, []);

  const handleCreated = useCallback(
    async (issued: IssuedKey) => {
      setRevealing(issued);
      setShowCreate(false);
      await refresh();
    },
    [refresh],
  );

  const handleRevoke = useCallback(
    async (id: string) => {
      setRevoking(id);
      setError(null);
      try {
        const res = await fetch(`/api/v1/keys/${id}`, { method: 'DELETE' });
        if (res.status === 204) {
          setKeys((prev) => prev.filter((k) => k.id !== id));
        } else {
          const body = await res.json().catch(() => ({}));
          setError(typeof body?.error?.override_hint === 'string' ? body.error.override_hint : `revoke failed: HTTP ${res.status}`);
        }
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err));
      } finally {
        setRevoking(null);
      }
    },
    [],
  );

  return (
    <div className="space-y-6">
      <div className="flex items-center justify-end">
        <button
          type="button"
          onClick={() => {
            setShowCreate(true);
            setError(null);
          }}
          className="rounded-md bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white"
          data-testid="create-key-button"
        >
          Create new key
        </button>
      </div>

      {error ? (
        <div className="rounded-lg border border-red-700/40 bg-red-900/20 p-3 text-sm text-red-200">
          {error}
        </div>
      ) : null}

      {revealing ? (
        <RevealPanel issued={revealing} onClose={() => setRevealing(null)} />
      ) : null}

      <KeysTable keys={keys} onRevoke={handleRevoke} revoking={revoking} />

      {showCreate ? (
        <CreateModal
          onClose={() => setShowCreate(false)}
          onCreated={handleCreated}
          onError={setError}
        />
      ) : null}
    </div>
  );
}

function KeysTable({
  keys,
  onRevoke,
  revoking,
}: {
  keys: KeyView[];
  onRevoke: (id: string) => Promise<void>;
  revoking: string | null;
}) {
  if (keys.length === 0) {
    return (
      <div className="rounded-lg border border-zinc-800 px-4 py-8 text-center text-sm text-zinc-500">
        No active keys. Create one to get started.
      </div>
    );
  }
  return (
    <div className="overflow-x-auto rounded-lg border border-zinc-800">
      <table className="min-w-full divide-y divide-zinc-800 text-sm">
        <thead className="bg-zinc-900/60 text-left text-xs uppercase tracking-wider text-zinc-400">
          <tr>
            <th className="px-4 py-2">name</th>
            <th className="px-4 py-2">prefix</th>
            <th className="px-4 py-2">scopes</th>
            <th className="px-4 py-2">created</th>
            <th className="px-4 py-2">last used</th>
            <th className="px-4 py-2 text-right">actions</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-zinc-800/80">
          {keys.map((k) => (
            <tr key={k.id} data-testid="key-row" data-key-id={k.id}>
              <td className="px-4 py-2">{k.name}</td>
              <td className="px-4 py-2 font-mono text-zinc-300">{k.prefix}…</td>
              <td className="px-4 py-2 font-mono text-xs text-zinc-400">
                {k.scopes.join(', ')}
              </td>
              <td className="px-4 py-2 text-zinc-400">
                {new Date(k.created_at).toLocaleString()}
              </td>
              <td className="px-4 py-2 text-zinc-400">
                {k.last_used_at ? new Date(k.last_used_at).toLocaleString() : 'never'}
              </td>
              <td className="px-4 py-2 text-right">
                <button
                  type="button"
                  onClick={() => {
                    if (window.confirm(`Revoke "${k.name}"? Requests using this key will start failing immediately.`)) {
                      void onRevoke(k.id);
                    }
                  }}
                  disabled={revoking === k.id}
                  className="rounded-md border border-red-800/60 px-3 py-1 text-xs text-red-300 hover:bg-red-900/30 disabled:opacity-50"
                  data-testid="revoke-button"
                >
                  {revoking === k.id ? 'revoking…' : 'Revoke'}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

function CreateModal({
  onClose,
  onCreated,
  onError,
}: {
  onClose: () => void;
  onCreated: (issued: IssuedKey) => void | Promise<void>;
  onError: (msg: string) => void;
}) {
  const [name, setName] = useState('');
  const [scopes, setScopes] = useState<Set<Scope>>(new Set<Scope>(['read']));
  const [submitting, setSubmitting] = useState(false);

  const toggleScope = (s: Scope) => {
    setScopes((prev) => {
      const next = new Set(prev);
      if (next.has(s)) next.delete(s);
      else next.add(s);
      return next;
    });
  };

  const onSubmit = async (e: React.FormEvent) => {
    e.preventDefault();
    if (submitting) return;
    setSubmitting(true);
    try {
      const res = await fetch('/api/v1/keys', {
        method: 'POST',
        headers: { 'content-type': 'application/json' },
        body: JSON.stringify({ name: name.trim(), scopes: Array.from(scopes) }),
      });
      if (!res.ok) {
        const body = await res.json().catch(() => ({}));
        onError(typeof body?.error?.override_hint === 'string' ? body.error.override_hint : `create failed: HTTP ${res.status}`);
        return;
      }
      const issued = (await res.json()) as IssuedKey;
      await onCreated(issued);
    } catch (err) {
      onError(err instanceof Error ? err.message : String(err));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div
      role="dialog"
      aria-modal="true"
      aria-labelledby="create-key-title"
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/60 p-4"
      onClick={onClose}
    >
      <form
        onSubmit={onSubmit}
        onClick={(e) => e.stopPropagation()}
        className="w-full max-w-md space-y-4 rounded-lg border border-zinc-700 bg-zinc-950 p-6 shadow-2xl"
      >
        <h2 id="create-key-title" className="text-lg font-semibold">
          Create new API key
        </h2>
        <label className="block text-sm">
          <span className="text-zinc-400">Name</span>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            required
            maxLength={128}
            placeholder="e.g. my-laptop or staging-bot"
            className="mt-1 w-full rounded-md border border-zinc-700 bg-zinc-900 px-3 py-2 text-sm focus:border-zinc-400 focus:outline-none"
            data-testid="key-name-input"
          />
        </label>
        <fieldset className="text-sm">
          <legend className="text-zinc-400">Scopes</legend>
          <div className="mt-2 space-y-1">
            {ALL_SCOPES.map((s) => (
              <label key={s} className="flex items-center gap-2 text-zinc-200">
                <input
                  type="checkbox"
                  checked={scopes.has(s)}
                  onChange={() => toggleScope(s)}
                  data-testid={`scope-${s}`}
                />
                <span className="font-mono">{s}</span>
              </label>
            ))}
          </div>
        </fieldset>
        <div className="flex justify-end gap-2 pt-2">
          <button
            type="button"
            onClick={onClose}
            className="rounded-md border border-zinc-700 px-3 py-1.5 text-sm hover:bg-zinc-900"
          >
            Cancel
          </button>
          <button
            type="submit"
            disabled={submitting || name.trim().length === 0 || scopes.size === 0}
            className="rounded-md bg-zinc-100 px-3 py-1.5 text-sm font-medium text-zinc-900 hover:bg-white disabled:opacity-50"
            data-testid="submit-create-key"
          >
            {submitting ? 'creating…' : 'Create'}
          </button>
        </div>
      </form>
    </div>
  );
}

function RevealPanel({ issued, onClose }: { issued: IssuedKey; onClose: () => void }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(issued.plaintext);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard may be unavailable on insecure origins; the input is still
       * selectable for a manual copy. */
    }
  };
  return (
    <div className="rounded-lg border border-amber-700/40 bg-amber-900/20 p-4 text-sm text-amber-100" data-testid="reveal-panel">
      <p className="font-semibold">Your new key — {issued.name}</p>
      <p className="mt-1 text-xs text-amber-200/80">
        This is the only time you&apos;ll see this key. Save it now.
      </p>
      <div className="mt-3 flex gap-2">
        <input
          readOnly
          value={issued.plaintext}
          className="w-full rounded-md border border-amber-700/40 bg-black/40 px-3 py-2 font-mono text-amber-100 focus:outline-none"
          onFocus={(e) => e.currentTarget.select()}
          data-testid="plaintext-input"
        />
        <button
          type="button"
          onClick={copy}
          className="shrink-0 rounded-md border border-amber-600/60 px-3 py-2 text-amber-100 hover:bg-amber-900/30"
        >
          {copied ? 'copied' : 'Copy'}
        </button>
      </div>
      <button
        type="button"
        onClick={onClose}
        className="mt-3 text-xs text-amber-200 underline-offset-2 hover:underline"
      >
        I&apos;ve saved it — dismiss
      </button>
    </div>
  );
}
