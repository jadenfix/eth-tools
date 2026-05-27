import Link from 'next/link';
import { auth, oauthEnabledHost } from '@/lib/auth';

export default async function DashboardHome() {
  const session = await auth();
  const oauthEnabled = await oauthEnabledHost();
  const login = session?.user?.githubLogin ?? session?.user?.name ?? 'guest';

  return (
    <main className="relative z-10 mx-auto max-w-4xl px-4 py-10 sm:px-6">
      {/* Top crumb */}
      <p className="mb-6 flex items-center gap-3 text-xs" style={{ color: 'var(--term-muted)' }}>
        <Link href="/" className="term-link">eth-tools</Link>
        <span>/</span>
        <span style={{ color: 'var(--term-accent-2)' }}>dashboard</span>
      </p>

      <section className="term-window">
        <div className="term-titlebar">
          <span className="term-dot term-dot-r" aria-hidden="true" />
          <span className="term-dot term-dot-y" aria-hidden="true" />
          <span className="term-dot term-dot-g" aria-hidden="true" />
          <span className="ml-3">~/dashboard — {login}@eth-tools</span>
        </div>
        <div className="term-body">
          <p>
            <span className="term-prompt-bare">$</span>{' '}
            <span style={{ color: 'var(--term-fg)' }}>whoami</span>
          </p>
          <p className="mt-2 text-sm" style={{ color: 'var(--term-fg-dim)' }}>
            {session?.user
              ? <>signed-in · github:<span style={{ color: 'var(--term-accent-2)' }}>{login}</span></>
              : 'unauthenticated'}
          </p>

          {!oauthEnabled ? (
            <div
              className="mt-6 rounded-md px-4 py-3 text-sm"
              style={{
                background: 'rgba(252, 211, 77, 0.08)',
                border: '1px solid rgba(252, 211, 77, 0.35)',
                color: 'var(--term-warn)',
              }}
            >
              <p>
                <span className="term-comment">OAuth disabled on PR previews</span>
              </p>
              <p className="mt-1" style={{ color: 'var(--term-fg-dim)' }}>
                Sign-in is disabled on PR previews. Use{' '}
                <a href="https://preview.eth-tools.dev" className="term-link">
                  preview.eth-tools.dev
                </a>{' '}
                or production.
              </p>
            </div>
          ) : session?.user ? (
            <>
              <hr className="term-divider" />
              <p className="text-sm" style={{ color: 'var(--term-fg-dim)' }}>
                <span className="term-comment">motd — pick a surface</span>
              </p>
              <ul className="mt-3 grid grid-cols-1 gap-2 text-sm sm:grid-cols-2">
                <DashLink href="/dashboard/keys"   cmd="keys list"      hint="API key issuance + revoke" />
                <DashLink href="/dashboard/agents" cmd="agents list"    hint="browse the indexed registry" />
                <DashLink href="/dashboard/workers" cmd="workers status" hint="cron telemetry · SLOs · lag" />
                <DashLink href="/dashboard/alerts"  cmd="alerts open"    hint="watchdog signals · ack queue" />
                <DashLink href="/dashboard/wallet"  cmd="wallet inspect" hint="env-var wallet · spend · rails" />
                <DashLink href="/docs"              cmd="man eth-tools"  hint="docs index" />
              </ul>
            </>
          ) : (
            <>
              <hr className="term-divider" />
              <p className="text-sm" style={{ color: 'var(--term-fg-dim)' }}>
                <span className="term-comment">sign in with github to issue an API key</span>
              </p>
              <div className="mt-4">
                <Link
                  href="/api/auth/signin?provider=github"
                  className="inline-flex items-center rounded-md px-4 py-2 text-sm font-medium"
                  style={{ background: 'var(--term-prompt)', color: 'var(--term-bg)' }}
                >
                  $ gh auth login
                </Link>
              </div>
            </>
          )}
        </div>
      </section>

      <p className="mt-6 text-xs" style={{ color: 'var(--term-muted)' }}>
        <span className="term-prompt-bare">$</span> exit — ^D to return to{' '}
        <Link href="/" className="term-link">eth-tools.dev</Link>
      </p>
    </main>
  );
}

function DashLink({ href, cmd, hint }: { href: string; cmd: string; hint: string }) {
  return (
    <li>
      <Link
        href={href}
        className="flex items-baseline gap-2 rounded-md px-3 py-2 transition-colors hover:bg-[rgba(192,132,252,0.08)]"
        style={{ border: '1px solid var(--term-border)', color: 'var(--term-fg-dim)' }}
      >
        <span className="term-prompt-bare">$</span>
        <span style={{ color: 'var(--term-accent-2)' }}>{cmd}</span>
        <span className="ml-auto text-xs" style={{ color: 'var(--term-muted)' }}>
          {hint}
        </span>
      </Link>
    </li>
  );
}
