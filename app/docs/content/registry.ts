// Static registry of docs pages.
//
// We hand-curate the slug list (rather than scandir at request time) so
// `app/docs/[slug]` can return a 404 for unknown slugs immediately and
// so the build doesn't need to know how to bundle every random file
// under content/. The MD body is loaded from disk on demand via
// `loadDocBody`.

import { promises as fs } from 'node:fs';
import path from 'node:path';

export interface DocEntry {
  slug: string;
  title: string;
  blurb: string;
}

export const DOCS: DocEntry[] = [
  {
    slug: 'quickstart',
    title: 'Quickstart',
    blurb: 'curl + npx + add-to-Claude in three commands.',
  },
  {
    slug: 'auth',
    title: 'Authentication',
    blurb: 'How to issue an API key and send the Bearer header.',
  },
  {
    slug: 'mcp',
    title: 'MCP server',
    blurb: 'Endpoint URL, RFC 9728 discovery, and the eight tools.',
  },
  {
    slug: 'wallet',
    title: 'Wallet rails',
    blurb: 'x402 USDC settlement on Base + the five hard rails.',
  },
  {
    slug: 'workers',
    title: 'Workers',
    blurb: 'What each worker does and the cron cadence.',
  },
];

export function findDoc(slug: string): DocEntry | undefined {
  return DOCS.find((d) => d.slug === slug);
}

// Slug validation: lowercase letters, digits, dashes only.
const VALID_SLUG = /^[a-z][a-z0-9-]*$/;

/**
 * Load a doc body from disk. Throws on path traversal or unknown slug —
 * keep the path-resolution checks in one place so we can audit them.
 */
export async function loadDocBody(slug: string): Promise<string> {
  if (!VALID_SLUG.test(slug)) {
    throw new Error(`invalid slug: ${slug}`);
  }
  // `process.cwd()` is the repo root in `next dev` and the project root
  // in `next start`. We anchor to `app/docs/content/<slug>.md`.
  const root = path.resolve(process.cwd(), 'app', 'docs', 'content');
  const file = path.resolve(root, `${slug}.md`);
  // Defence in depth: the resolved path must remain under `root`.
  if (!file.startsWith(root + path.sep)) {
    throw new Error(`path traversal blocked: ${slug}`);
  }
  return fs.readFile(file, 'utf8');
}
