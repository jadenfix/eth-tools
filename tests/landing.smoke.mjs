// Landing-page smoke test.
//
// Why not Playwright? The Phase-8 brief forbids adding heavy npm deps;
// Playwright + browser binaries run ~300 MB. Instead we run a copy-and-shape
// smoke over the source files. This catches:
//
//   - Removal/regression of the required hero copy.
//   - Missing CTAs, missing footer links, missing tab labels.
//   - Featured-agents fixture drifting away from the shape `page.tsx` expects.
//
// It does NOT catch runtime rendering bugs. A render-time smoke can be added
// once we wire `react-dom/server` against the App-Router server-component
// boundary (non-trivial without `next test`).

import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

const read = (rel) => readFile(path.join(root, rel), 'utf8');

test('landing hero contains the required headline + subhead', async () => {
  const src = await read('app/page.tsx');
  assert.match(src, /The self-maintaining runtime for ERC-8004 agents/);
  assert.match(src, /Discoverable, validated, live, paid — and maintained by other agents\./);
});

test('landing hero exposes both primary CTAs', async () => {
  const src = await read('app/page.tsx');
  assert.match(src, /Get an API key/);
  assert.match(src, /href=["']\/dashboard\/keys["']/);
  assert.match(src, /View the docs/);
  assert.match(src, /href=["']\/docs["']/);
});

test('live counters component fetches /api/v1/health', async () => {
  const src = await read('app/_landing/LiveCounters.tsx');
  assert.match(src, /\/api\/v1\/health/);
  assert.match(src, /agents indexed/);
  assert.match(src, /manifests validated in last 24h/);
  assert.match(src, /seconds since last on-chain event/);
});

test('try-it section exposes curl, npx, and add-to-claude tabs', async () => {
  const src = await read('app/_landing/TryIt.tsx');
  assert.match(src, /curl https:\/\/eth-tools\.dev\/api\/v1\/agents \| jq/);
  assert.match(src, /npx -y eth-tools find "wallet risk"/);
  assert.match(src, /Add to Claude/);
  assert.match(src, /claude_desktop_config\.json/);
  assert.match(src, /"mcpServers"/);
});

test('what-it-is section names all three product surfaces', async () => {
  const src = await read('app/page.tsx');
  assert.match(src, /Registry indexer/);
  assert.match(src, /MCP server/);
  assert.match(src, /Paid-write relay/);
  assert.match(src, /5 hard rails/);
});

test('footer carries all required links', async () => {
  const src = await read('app/page.tsx');
  for (const pattern of [
    /github\.com\/jadenfix\/eth-tools/,
    /href=["']\/docs["']/,
    /href=["']\/dashboard\/workers["']/,
    /twitter\.com\/eth_tools/i,
    /discord/i,
  ]) {
    assert.match(src, pattern);
  }
});

test('well-known-agents fixture matches the landing-page schema', async () => {
  const raw = JSON.parse(await read('fixtures/well-known-agents.json'));
  assert.ok(Array.isArray(raw), 'fixture must be an array');
  assert.ok(raw.length >= 3 && raw.length <= 12, 'fixture should have 3-12 entries');
  for (const entry of raw) {
    assert.equal(typeof entry.chain, 'string');
    assert.equal(typeof entry.agentId, 'string');
    assert.equal(typeof entry.label, 'string');
    assert.equal(typeof entry.description, 'string');
    assert.equal(typeof entry.manifestUrl, 'string');
  }
});

test('README quickstart covers all three surfaces', async () => {
  const src = await read('README.md');
  assert.match(src, /curl https:\/\/eth-tools\.dev\/api\/v1\/agents/);
  assert.match(src, /npx -y eth-tools/);
  assert.match(src, /mcpServers/);
  assert.match(src, /Not a new payment standard/);
  assert.match(src, /Not a UI fork/);
  assert.match(src, /Not a TEE/);
});

test('AGENTS.md documents the hot path and the three how-tos', async () => {
  const src = await read('AGENTS.md');
  assert.match(src, /## The hot path/);
  assert.match(src, /## How to add a new HTTP endpoint/);
  assert.match(src, /## How to add a new MCP tool/);
  assert.match(src, /## How to run tests/);
  assert.match(src, /cargo test --workspace/);
  assert.match(src, /Don't log secrets/);
});

test('IDE rules carry the same context as AGENTS.md', async () => {
  const claude = await read('.claude/commands/eth-tools.md');
  const cursor = await read('.cursor/rules/eth-tools.mdc');
  for (const src of [claude, cursor]) {
    assert.match(src, /## Hot path/);
    assert.match(src, /## How to add a new HTTP endpoint/);
    assert.match(src, /## How to add a new MCP tool/);
    assert.match(src, /## How to run tests/);
    assert.match(src, /wallet rails/);
  }
});
