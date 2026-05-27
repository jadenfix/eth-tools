#!/usr/bin/env bash
# scripts/bootstrap.sh — one-command local dev setup.
# Idempotent; safe to re-run.

set -euo pipefail

cd "$(dirname "$0")/.."

echo "==> Enabling pnpm via corepack"
if command -v corepack >/dev/null 2>&1; then
  corepack enable
  corepack prepare pnpm@10 --activate
else
  echo "corepack not found — install Node.js >= 20.10 from nodejs.org"
  exit 1
fi

echo "==> Installing node deps"
pnpm install --frozen-lockfile || pnpm install

echo "==> Installing sqlx-cli (if missing)"
if ! command -v sqlx >/dev/null 2>&1; then
  cargo install sqlx-cli --version "^0.8" --no-default-features --features postgres,rustls
fi

echo "==> Starting docker compose (postgres + dragonfly)"
docker compose up -d postgres dragonfly

echo "==> Waiting for postgres to be ready"
for i in {1..30}; do
  if docker compose exec -T postgres pg_isready -U dev -d eth_tools >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if [ ! -f .env.local ]; then
  echo "==> Creating .env.local from .env.example"
  cp .env.example .env.local
fi

echo "==> Running migrations"
DATABASE_URL="postgres://dev:dev@localhost:5432/eth_tools?sslmode=disable" \
  sqlx migrate run --source crates/db/migrations || true

echo "==> Generating OpenAPI + TypeScript types"
pnpm gen:types || true

cat <<EOF

==> Bootstrap complete.

Next:
  Terminal A:  cargo run -p eth-tools-dev-server     # Rust API on :3000
  Terminal B:  pnpm dev                              # Next.js dashboard on :3001
  Verify:      curl http://localhost:3000/ | jq

EOF
