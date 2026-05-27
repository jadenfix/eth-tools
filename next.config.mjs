/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Avoid Next.js trying to read /api/cron/*.rs or /api/v1/[...path].rs as routes.
  // Those are Vercel Rust functions; Next.js owns only app/api/*.
  outputFileTracingExcludes: {
    '*': ['./api/**', './crates/**', './target/**'],
  },
};

export default nextConfig;
