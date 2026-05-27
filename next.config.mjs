import { dirname } from 'node:path';
import { fileURLToPath } from 'node:url';

const __dirname = dirname(fileURLToPath(import.meta.url));

/** @type {import('next').NextConfig} */
const nextConfig = {
  reactStrictMode: true,
  // Multiple worktrees create stray lockfiles up the tree (e.g. ~/pnpm-lock.yaml).
  // Pin the workspace root to this repo so Next.js doesn't trace the wrong tree
  // (the build warned about /Users/jadenfix/pnpm-lock.yaml without this).
  outputFileTracingRoot: __dirname,
  // Avoid Next.js trying to read /api/cron/*.rs or /api/v1/[...path].rs as routes.
  // Those are Vercel Rust functions; Next.js owns only app/api/*.
  outputFileTracingExcludes: {
    '*': ['./api/**', './crates/**', './target/**'],
  },
};

export default nextConfig;
