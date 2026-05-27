// Auth.js v5 (NextAuth) configuration.
//
// Plan §8.1: GitHub provider, JWT session strategy.
// Plan §11.5 OAuth row: PR-preview deployments (`*-git-*.vercel.app`) are
// read-only — the OAuth flow is disabled so we don't hit GitHub's "callback
// URL mismatch" trap. The dashboard shows a banner instead.

import NextAuth, { type DefaultSession } from 'next-auth';
// The side-effect import forces TS to resolve the submodule so the
// `declare module` augmentation below targets a known module.
import 'next-auth/jwt';
import GitHub from 'next-auth/providers/github';

// Augment Session / JWT so `session.user.githubLogin` and `token.login`
// are typed under strict mode. Without this, callbacks below would not
// type-check.
declare module 'next-auth' {
  interface Session {
    user: {
      id: string;
      githubLogin?: string;
    } & DefaultSession['user'];
  }
}

declare module 'next-auth/jwt' {
  interface JWT {
    login?: string;
  }
}

/**
 * The custom-domain alias assigned to the `main` branch's preview deployment
 * (Vercel "Branch Alias"). The second GitHub OAuth App's callback is registered
 * here; we let sign-in through on requests that arrive at this host.
 */
export const STABLE_PREVIEW_ALIAS = 'preview.eth-tools.dev';

/**
 * Returns true on hosts where the GitHub OAuth flow is permitted.
 *
 * - local dev (no VERCEL_ENV): true.
 * - production (VERCEL_ENV === 'production'): true.
 * - preview deploys: true iff the inbound request's `Host` header equals
 *   {@link STABLE_PREVIEW_ALIAS}. The reason this checks the Host header (not
 *   `VERCEL_URL`): on Vercel `VERCEL_URL` is always the per-deployment
 *   `*-git-*.vercel.app` URL, never the assigned custom alias, so the
 *   previous `VERCEL_URL === 'preview.eth-tools.dev'` check was dead code.
 *   PR previews and any preview hit through `*.vercel.app`: FALSE — banner
 *   instead.
 *
 * Reads `headers()` so it must be invoked from a Server Component, Route
 * Handler, or Server Action.
 */
export async function oauthEnabledHost(): Promise<boolean> {
  const env = process.env.VERCEL_ENV;
  if (!env) return true; // local dev
  if (env === 'production') return true;

  // Defer the dynamic import so this module remains importable from places
  // that don't run inside a request scope (e.g. NextAuth config bootstrap).
  const { headers } = await import('next/headers');
  const h = await headers();
  const host = h.get('host')?.toLowerCase() ?? '';
  return host === STABLE_PREVIEW_ALIAS;
}

export const { handlers, auth, signIn, signOut } = NextAuth({
  providers: [GitHub],
  session: { strategy: 'jwt' },
  callbacks: {
    async session({ session, token }) {
      if (token.sub) {
        session.user.id = token.sub;
      }
      if (typeof token.login === 'string') {
        session.user.githubLogin = token.login;
      }
      return session;
    },
    async jwt({ token, profile }) {
      // GitHub's profile carries the username under `login`.
      if (profile && typeof (profile as { login?: unknown }).login === 'string') {
        token.login = (profile as { login: string }).login;
      }
      return token;
    },
  },
});
