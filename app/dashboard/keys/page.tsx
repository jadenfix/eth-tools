// /dashboard/keys — server component, gates on session and renders the
// client-side keys manager. Server-side initial fetch avoids a flash of
// empty state.

import Link from 'next/link';
import { auth, oauthEnabledHost } from '@/lib/auth';
import { listKeys, type KeyView } from '@/lib/keys';
import { KeysClient } from './keys-client';

export const dynamic = 'force-dynamic';

export default async function KeysPage() {
  const session = await auth();
  const oauthEnabled = await oauthEnabledHost();

  if (!oauthEnabled) {
    return (
      <main className="mx-auto max-w-3xl px-6 py-12">
        <h1 className="text-2xl font-semibold tracking-tight">API keys</h1>
        <div className="mt-6 rounded-lg border border-amber-700/40 bg-amber-900/20 p-4 text-sm text-amber-200">
          <p className="font-medium">OAuth disabled on PR previews</p>
          <p className="mt-2">
            Sign-in is disabled on PR previews; use{' '}
            <a href="https://preview.eth-tools.dev" className="underline underline-offset-2">
              https://preview.eth-tools.dev
            </a>{' '}
            or production.
          </p>
        </div>
      </main>
    );
  }

  if (!session?.user) {
    return (
      <main className="mx-auto max-w-3xl px-6 py-12">
        <h1 className="text-2xl font-semibold tracking-tight">API keys</h1>
        <p className="mt-4 text-sm text-zinc-400">
          You need to be signed in to issue API keys.
        </p>
        <Link
          href="/api/auth/signin?provider=github&callbackUrl=/dashboard/keys"
          className="mt-4 inline-flex items-center rounded-md bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white"
        >
          Sign in with GitHub
        </Link>
      </main>
    );
  }

  let initial: KeyView[] = [];
  let loadError: string | null = null;
  try {
    initial = await listKeys({
      userId: session.user.id,
      login: session.user.githubLogin ?? session.user.name ?? '',
    });
  } catch (err) {
    loadError = err instanceof Error ? err.message : String(err);
  }

  return (
    <main className="mx-auto max-w-4xl px-6 py-10">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-2xl font-semibold tracking-tight">API keys</h1>
        <Link href="/dashboard" className="text-sm text-zinc-400 underline-offset-2 hover:underline">
          ← Dashboard
        </Link>
      </div>

      <p className="mb-6 text-sm text-zinc-400">
        Use these bearer tokens to authenticate requests to the eth-tools HTTP and MCP APIs.
        Format: <code className="font-mono text-zinc-300">et_live_xxxx</code> in production,{' '}
        <code className="font-mono text-zinc-300">et_test_xxxx</code> in previews and dev.
        Tokens are hashed with bcrypt; the plaintext is shown only at issuance — copy it then.
      </p>

      {loadError ? (
        <div className="mb-6 rounded-lg border border-red-700/40 bg-red-900/20 p-4 text-sm text-red-200">
          <p className="font-medium">Could not load keys</p>
          <p className="mt-2 font-mono text-xs">{loadError}</p>
        </div>
      ) : null}

      <KeysClient initial={initial} />
    </main>
  );
}
