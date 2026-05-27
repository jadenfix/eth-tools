// Auth.js v5 middleware. Gates `/dashboard/*` behind a valid session.
// Unauthenticated visitors are redirected to the GitHub OAuth sign-in flow
// with the original path preserved as `callbackUrl` so they land back where
// they started.

import { auth } from '@/lib/auth';
import { NextResponse } from 'next/server';

export default auth((req) => {
  if (req.auth) {
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
