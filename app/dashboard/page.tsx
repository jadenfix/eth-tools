import Link from 'next/link';
import { auth, oauthEnabledHost } from '@/lib/auth';

export default async function DashboardHome() {
  const session = await auth();
  const oauthEnabled = await oauthEnabledHost();

  return (
    <main className="mx-auto max-w-3xl px-6 py-12">
      <h1 className="text-3xl font-semibold tracking-tight">Dashboard</h1>

      {!oauthEnabled ? (
        <div className="mt-6 rounded-lg border border-amber-700/40 bg-amber-900/20 p-4 text-sm text-amber-200">
          <p className="font-medium">OAuth disabled on PR previews</p>
          <p className="mt-2">
            Sign-in is disabled on PR previews; use{' '}
            <a
              href="https://preview.eth-tools.dev"
              className="underline underline-offset-2"
            >
              https://preview.eth-tools.dev
            </a>{' '}
            or production.
          </p>
        </div>
      ) : session?.user ? (
        <div className="mt-6 space-y-6">
          <p className="text-lg">
            Welcome, <span className="font-medium">{session.user.githubLogin ?? session.user.name ?? 'agent'}</span>.
          </p>
          <ul className="space-y-2 text-sm">
            <li>
              <Link href="/dashboard/agents" className="underline underline-offset-2">
                Browse indexed agents
              </Link>
            </li>
            <li>
              <Link href="/dashboard/wallet" className="underline underline-offset-2">
                Wallet — balance, spend, denials
              </Link>
            </li>
            <li>
              <Link href="/dashboard/workers" className="underline underline-offset-2">
                Workers — SLO + cursor lag
              </Link>
            </li>
            <li>
              <Link href="/dashboard/alerts" className="underline underline-offset-2">
                Alerts — triage queue
              </Link>
            </li>
            <li>
              <Link href="/dashboard/keys" className="underline underline-offset-2">
                API keys (coming soon)
              </Link>
            </li>
            <li>
              <Link href="/docs" className="underline underline-offset-2">
                Docs
              </Link>
            </li>
          </ul>
        </div>
      ) : (
        <div className="mt-6 space-y-4">
          <p className="text-sm text-zinc-400">
            Sign in with GitHub to issue an API key and manage your agents.
          </p>
          <Link
            href="/api/auth/signin?provider=github"
            className="inline-flex items-center rounded-md bg-zinc-100 px-4 py-2 text-sm font-medium text-zinc-900 hover:bg-white"
          >
            Sign in with GitHub
          </Link>
        </div>
      )}
    </main>
  );
}
