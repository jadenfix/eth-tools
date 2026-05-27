// Typed fetcher to the Rust API. Wraps fetch() with the same-project rewrite.
// Full implementation lands when the dashboard starts consuming /api/v1/* in Phase 3.

import type { paths } from './api-types';

export type ApiPaths = paths;

export async function api<P extends keyof paths>(_path: P): Promise<unknown> {
  throw new Error('api(): typed fetcher lands in Phase 3');
}
