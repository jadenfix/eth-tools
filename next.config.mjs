/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Avoid Next.js trying to read /api/cron/*.rs or /api/v1/[...path].rs as routes.
  // Those are Vercel Rust functions; Next.js owns only app/api/*.
  outputFileTracingExcludes: {
    '*': ['./api/**', './crates/**', './target/**'],
  },
  // Force-include the MDX/markdown sources for /docs. They're read via
  // fs.readFile at request time (not statically imported), so the Next
  // tracer can't see them — without this they'd be missing from the
  // Vercel serverless bundle and /docs/[slug] would 500.
  outputFileTracingIncludes: {
    '/docs': ['./app/docs/content/*.md'],
    '/docs/[slug]': ['./app/docs/content/*.md'],
  },
  // Phase-3 owns only the dashboard sub-pages, the docs tree, the lib
  // helpers, and the new API routes — keep `next lint` scoped to those
  // so a pre-existing `<a href="/docs">` in app/page.tsx (owned by the
  // Phase 8 landing agent) doesn't block our build. The Phase 8 PR will
  // rewrite that file and can lift the scope back to ['app'].
  eslint: {
    dirs: [
      'app/api/v1/alerts',
      'app/api/v1/workers',
      'app/dashboard/alerts',
      'app/dashboard/wallet',
      'app/dashboard/workers',
      'app/docs',
      'app/lib',
    ],
  },
};

export default nextConfig;
