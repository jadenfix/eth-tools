// /docs — public index of operator-facing documentation.
//
// No auth required — these pages are intentionally crawlable. They live
// inside the app/ tree (rather than as static files) so they can link to
// dashboard URLs and benefit from the global layout.

import Link from 'next/link';
import { DOCS } from './content/registry';

export const dynamic = 'force-static';

export default function DocsIndex() {
  return (
    <main className="mx-auto max-w-3xl px-6 py-12">
      <h1 className="text-3xl font-semibold tracking-tight">Docs</h1>
      <p className="mt-2 text-zinc-400">
        Operator-facing guides. For the typed API surface see{' '}
        <a
          href="/openapi.json"
          className="underline underline-offset-2"
        >
          /openapi.json
        </a>
        .
      </p>

      <ul className="mt-8 space-y-4">
        {DOCS.map((doc) => (
          <li
            key={doc.slug}
            className="rounded-lg border border-zinc-800 p-4 hover:bg-zinc-900/30"
          >
            <Link href={`/docs/${doc.slug}`} className="block">
              <h2 className="text-lg font-medium">{doc.title}</h2>
              <p className="mt-1 text-sm text-zinc-400">{doc.blurb}</p>
            </Link>
          </li>
        ))}
      </ul>
    </main>
  );
}
