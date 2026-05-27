'use client';

// Minimal manual-refresh control. We deliberately do NOT auto-poll the
// wallet page — RPC is rate-limited upstream and the balance keeper cron
// is the canonical refresher. Operators trigger a full reload when they
// need fresh data; this button keeps the affordance discoverable.

import { useRouter } from 'next/navigation';
import { useTransition } from 'react';

export function RefreshButton() {
  const router = useRouter();
  const [pending, startTransition] = useTransition();

  return (
    <button
      type="button"
      disabled={pending}
      onClick={() =>
        startTransition(() => {
          router.refresh();
        })
      }
      className="inline-flex items-center rounded-md border border-zinc-700 px-3 py-1 text-xs text-zinc-200 hover:bg-zinc-900 disabled:opacity-50"
    >
      {pending ? 'refreshing…' : 'refresh'}
    </button>
  );
}
