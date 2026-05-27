// Auth.js v5 middleware. Gates `/dashboard/*` behind a valid session.
//
// Two distinct paths:
//
// 1. **OAuth-disabled hosts** (PR-preview `*-git-*.vercel.app`, local hosts
//    not on the stable alias): let the request through unauthenticated so the
//    page-level banner in `app/dashboard/page.tsx` renders. Redirecting here
//    would kick off GitHub OAuth → mismatched-callback-URL → user sees an
//    error page instead of "PR preview is read-only". This is the bug that
//    defeated the previous `oauthEnabledHost()` gate: middleware fires before
//    the page, so the banner-level check was dead code.
// 2. **OAuth-enabled hosts** (production + the stable `preview.eth-tools.dev`
//    alias): unauthenticated visitors get redirected to the GitHub OAuth flow
//    with the original path preserved as `callbackUrl`.
//
// The host gate intentionally lives in middleware (edge runtime) so we never
// hand a redirect to a host where the callback is guaranteed to fail.

import { STABLE_PREVIEW_ALIAS } from '@/lib/auth';
import { auth } from '@/lib/auth';
import { NextResponse } from 'next/server';

/**
 * Edge-runtime variant of `oauthEnabledHost()` — duplicated here because
 * `app/lib/auth.ts::oauthEnabledHost` uses `next/headers` (Server Component
 * scope) which is not available in middleware. Middleware reads the request
 * Host header directly off `req.headers`. Keep the two implementations in
 * lockstep — the production OAuth callback is registered for both hosts.
 */
function isOauthEnabledHost(host: string | null): boolean {
  const env = process.env.VERCEL_ENV;
  if (!env) return true; // local dev
  if (env === 'production') return true;
  return host?.toLowerCase() === STABLE_PREVIEW_ALIAS;
}

export default auth((req) => {
  if (req.auth) {
    return NextResponse.next();
  }

  const host = req.headers.get('host');
  if (!isOauthEnabledHost(host)) {
    // Banner-mode: render the page so the user sees "PR preview is
    // read-only — use the production dashboard." Never redirect to OAuth
    // here; the callback URL won't match and the user dead-ends on a
    // GitHub error page.
    return NextResponse.next();
  }

  const url = req.nextUrl.clone();
  const callbackUrl = req.nextUrl.pathname + req.nextUrl.search;
  url.pathname = '/api/auth/signin';
  url.search = `?callbackUrl=${encodeURIComponent(callbackUrl)}`;
  return NextResponse.redirect(url);
});

export const config = {
  matcher: ['/dashboard/:path*'],
};
