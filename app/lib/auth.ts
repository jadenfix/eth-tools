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
 * Returns true on hosts where the GitHub OAuth flow is permitted.
 *
 * - production: always true.
 * - the stable preview alias `preview.eth-tools.dev`: true (second OAuth App).
 * - local dev (no VERCEL_ENV): true.
 * - PR previews (`*-git-*.vercel.app`, VERCEL_ENV === 'preview' without the
 *   stable alias): FALSE — show the disabled banner instead of attempting
 *   sign-in.
 */
export function oauthEnabledHost(): boolean {
  const env = process.env.VERCEL_ENV;
  if (!env) return true; // local dev
  if (env === 'production') return true;
  if (process.env.VERCEL_URL === 'preview.eth-tools.dev') return true;
  return false;
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
