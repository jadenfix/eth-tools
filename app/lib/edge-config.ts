// Thin read-only wrapper around Vercel Edge Config for the dashboard.
//
// We don't import `@vercel/edge-config` to keep the dep surface small —
// the public Edge Config read API is a single HTTPS GET. The connection
// string is exposed as `EDGE_CONFIG=https://edge-config.vercel.com/<id>?token=<…>`.
//
// All reads are best-effort: if Edge Config isn't configured or returns a
// non-2xx, we return `null` and let pages render an "unknown" state. This
// matches the operator workflow — they toggle the switch via the Vercel
// CLI, and the dashboard simply reflects whatever it sees.

const EDGE_CONFIG_REVALIDATE_SECONDS = 10;

export async function readEdgeConfigBoolean(
  key: string,
): Promise<boolean | null> {
  const conn = process.env.EDGE_CONFIG;
  if (!conn) return null;

  let url: URL;
  try {
    // Connection-string shape:
    //   https://edge-config.vercel.com/<config_id>?token=<token>
    // REST item read:
    //   https://edge-config.vercel.com/<config_id>/item/<key>?token=<token>
    //
    // We build the target URL manually rather than via
    // `new URL(relative, base)` — the relative-URL constructor collapses
    // the connection's query string onto the base path in surprising
    // ways (the token ends up on the wrong side of the path join). The
    // manual build keeps the config id intact and the token on the new
    // request's query string.
    const base = new URL(conn);
    const token = base.searchParams.get('token');
    if (!token) return null;
    const configId = base.pathname.replace(/^\/+/, '').replace(/\/+$/, '');
    if (!configId || configId.includes('/')) return null;
    url = new URL(
      `/${encodeURIComponent(configId)}/item/${encodeURIComponent(key)}`,
      `${base.protocol}//${base.host}`,
    );
    url.searchParams.set('token', token);
  } catch {
    return null;
  }

  try {
    const res = await fetch(url.toString(), {
      method: 'GET',
      headers: { accept: 'application/json' },
      // ISR-style cache: avoid hammering Edge Config on every request render.
      next: { revalidate: EDGE_CONFIG_REVALIDATE_SECONDS },
    });
    if (!res.ok) return null;
    const value: unknown = await res.json();
    if (typeof value === 'boolean') return value;
    return null;
  } catch {
    return null;
  }
}
