// Landing-page smoke test.
//
// Why not Playwright? The Phase-8 brief forbids adding heavy npm deps;
// Playwright + browser binaries run ~300 MB. Instead we run a copy-and-shape
// smoke over the source files. This catches:
//
//   - Removal/regression of the required hero copy and aesthetic primitives.
//   - Missing CTAs, missing footer links, missing tab labels.
//   - Featured-agents fixture drifting away from the shape `page.tsx` expects.
//   - README / AGENTS.md / IDE-rule drift from the agent-first messaging.
//
// It does NOT catch runtime rendering bugs.

import { readFile } from 'node:fs/promises';
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { fileURLToPath } from 'node:url';
import path from 'node:path';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '..');

const read = (rel) => readFile(path.join(root, rel), 'utf8');

test('landing hero contains the required headline + agent-first framing', async () => {
  const src = await read('app/page.tsx');
  assert.match(src, /The self-maintaining runtime for ERC-8004 agents/);
  // Agent-first framing — three machine surfaces + maintained-by-agents claim
  assert.match(src, /Maintained by other agents/i);
  assert.match(src, /Three machine surfaces/i);
});

test('landing carries the terminal aesthetic primitives', async () => {
  const src = await read('app/page.tsx');
  // The visual theme is a hard requirement, not a default. Every page must
  // build with the term-* primitives.
  assert.match(src, /term-window/);
  assert.match(src, /term-titlebar/);
  assert.match(src, /term-prompt-bare/);
  assert.match(src, /term-cursor/);
});

test('landing exposes machine-surface navigation', async () => {
  const src = await read('app/page.tsx');
  // No more big marketing CTA buttons — the page is terminal-style with
  // a top nav of agent-discoverable surfaces.
  assert.match(src, /href=["']\/docs["']/);
  assert.match(src, /href=["']\/openapi\.json["']/);
  assert.match(src, /href=["']\/llms\.txt["']/);
  assert.match(src, /href=["']\/dashboard["']/);
});

test('live counters component fetches /api/v1/health', async () => {
  const src = await read('app/_landing/LiveCounters.tsx');
  assert.match(src, /\/api\/v1\/health/);
  // Terminal-styled labels — uppercase, underscored, no marketing copy.
  // STATUS · AGENTS · CHAINS · WORKERS · MANIFESTS/24h · LAST EVENT.
  assert.match(src, /label="AGENTS"/);
  assert.match(src, /label="CHAINS"/);
  assert.match(src, /label="MANIFESTS\/24h"/);
  assert.match(src, /label="LAST EVENT"/);
  assert.match(src, /indexed/i);
});

test('try-it section exposes curl, npx, claude, and cursor surfaces', async () => {
  const src = await read('app/_landing/TryIt.tsx');
  assert.match(src, /curl[^\n]*https:\/\/eth-tools\.dev\/api\/v1\/agents/);
  assert.match(src, /npx -y eth-tools find "wallet risk"/);
  assert.match(src, /add to claude/i);
  assert.match(src, /add to cursor/i);
  assert.match(src, /claude_desktop_config\.json/);
  assert.match(src, /\.cursor\/mcp\.json/);
  assert.match(src, /"mcpServers"/);
});

test('landing names all three product surfaces and the wallet rails', async () => {
  const src = await read('app/page.tsx');
  assert.match(src, /registry indexer/i);
  assert.match(src, /MCP server/i);
  assert.match(src, /paid-write relay/i);
  // Five hard rails are the agent-first security spine.
  // `\s+` not literal space — JSX wraps long lines on whitespace boundaries.
  assert.match(src, /five hard\s+rails/i);
});

test('footer carries the agent-discoverable links', async () => {
  const src = await read('app/page.tsx');
  for (const pattern of [
    /github\.com\/jadenfix\/eth-tools/,
    /href=["']\/dashboard\/workers["']/,
    /twitter\.com\/eth_tools/i,
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

test('README quickstart covers all three surfaces + what-it-isn\'t', async () => {
  const src = await read('README.md');
  assert.match(src, /curl[^\n]*https:\/\/eth-tools\.dev\/api\/v1\/agents/);
  assert.match(src, /npx -y eth-tools/);
  assert.match(src, /mcpServers/);
  assert.match(src, /agent-first/i);
  assert.match(src, /Not a new payment standard/);
  assert.match(src, /Not a UI fork/);
  assert.match(src, /Not TEE/);
});

test('AGENTS.md documents the hot path, the how-tos, and the don\'ts', async () => {
  const src = await read('AGENTS.md');
  assert.match(src, /## The hot path/);
  assert.match(src, /## How to add a new HTTP endpoint/);
  assert.match(src, /## How to add a new MCP tool/);
  assert.match(src, /## How to add a new cron worker/);
  assert.match(src, /## How to add a dashboard page/);
  assert.match(src, /## How to run tests/);
  assert.match(src, /cargo test --workspace/);
  assert.match(src, /Don't log secrets/);
  // Agent-first framing — the dashboard is one more machine surface.
  assert.match(src, /agent-first/i);
  assert.match(src, /terminal/i);
});

test('IDE rules carry the hot-path context and the wallet-rails don\'t', async () => {
  const claude = await read('.claude/commands/eth-tools.md');
  const cursor = await read('.cursor/rules/eth-tools.mdc');
  for (const src of [claude, cursor]) {
    assert.match(src, /## Hot path/);
    assert.match(src, /## How to add a new HTTP endpoint/);
    assert.match(src, /## How to add a new MCP tool/);
    assert.match(src, /## How to add a new cron worker/);
    assert.match(src, /## How to run tests/);
    assert.match(src, /wallet rails/i);
    assert.match(src, /terminal/i);
  }
});

test('llms.txt enumerates the HTTP + MCP + CLI surfaces', async () => {
  const src = await read('app/llms.txt/route.ts');
  assert.match(src, /\/api\/v1\/agents/);
  assert.match(src, /\/api\/mcp/);
  assert.match(src, /npx -y eth-tools/);
  assert.match(src, /oauth-protected-resource/);
  assert.match(src, /llmstxt\.org/);
});

test('.well-known/eth-tools.json declares the surfaces and contracts', async () => {
  const src = await read('app/.well-known/eth-tools.json/route.ts');
  assert.match(src, /surfaces:/);
  // Tools split into shipped vs planned so agents that crawl the manifest
  // never see method-not-found for surfaces we advertise.
  assert.match(src, /tools_available/);
  assert.match(src, /tools_planned/);
  assert.match(src, /0x8004A169/);
  assert.match(src, /0x8004BAa1/);
  assert.match(src, /0x8004Cc84/);
});

test('docs page indexes the machine surfaces and topics', async () => {
  const src = await read('app/docs/page.tsx');
  assert.match(src, /\/api\/v1\/health/);
  assert.match(src, /\/api\/mcp/);
  assert.match(src, /\/openapi\.json/);
  assert.match(src, /\/llms\.txt/);
  assert.match(src, /oauth-protected-resource/);
  assert.match(src, /eth-tools manifest/);
});
