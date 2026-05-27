// /docs/[slug] — render a single Markdown document.
//
// Public. We use react-markdown (no MDX) to keep the build cheap and
// avoid pulling JSX into untrusted-ish content. The content is owned
// by us today, but the small dep surface still pays off.

import Link from 'next/link';
import { notFound } from 'next/navigation';
import Markdown from 'react-markdown';
import { DOCS, findDoc, loadDocBody } from '../content/registry';

// All slugs are known at build time → SSG for free, and an unknown
// slug 404s before hitting the FS.
export function generateStaticParams() {
  return DOCS.map((d) => ({ slug: d.slug }));
}

export const dynamicParams = false;

export default async function DocPage({
  params,
}: {
  params: Promise<{ slug: string }>;
}) {
  const { slug } = await params;
  const meta = findDoc(slug);
  if (!meta) {
    notFound();
  }

  let body: string;
  try {
    body = await loadDocBody(slug);
  } catch {
    notFound();
  }

  return (
    <main className="mx-auto max-w-3xl px-6 py-12">
      <div className="mb-6 flex items-baseline justify-between">
        <h1 className="text-3xl font-semibold tracking-tight">{meta.title}</h1>
        <Link
          href="/docs"
          className="text-sm text-zinc-400 underline-offset-2 hover:underline"
        >
          ← All docs
        </Link>
      </div>
      <article className="prose prose-invert prose-zinc max-w-none prose-pre:bg-zinc-900 prose-pre:text-zinc-100 prose-code:font-mono prose-a:underline-offset-2">
        <Markdown>{body}</Markdown>
      </article>
    </main>
  );
}
