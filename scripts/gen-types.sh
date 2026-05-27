#!/usr/bin/env bash
# scripts/gen-types.sh — regenerate openapi.json + api-types.ts.
# CI runs this and asserts no diff against committed files.

set -euo pipefail
cd "$(dirname "$0")/.."

mkdir -p app/lib public
cargo run --quiet -p eth-tools-openapi-gen > public/openapi.json
pnpm exec openapi-typescript public/openapi.json --output app/lib/api-types.ts
