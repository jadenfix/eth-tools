# Summary

<!-- 1-3 bullets describing what changes and why. -->

# Plan reference

<!-- Which section(s) of /Users/jadenfix/.claude/plans/let-s-do-8004-buzzing-bear.md does this PR implement? -->

# Test plan

<!-- Checklist of how you verified this works. Required: CI green. -->

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace`
- [ ] `pnpm typecheck`
- [ ] `pnpm gen:types` produces no diff
- [ ] Manual smoke test (describe what you ran)

# Security checklist

- [ ] No secret committed (search diff for `EVM_PRIVATE_KEY`, `AUTH_SECRET`, bearer tokens)
- [ ] No log statement prints `std::env::var(...)` results
- [ ] Migrations are forward+backward compatible (or explicitly flagged)
- [ ] Wallet rails (plan §10.2) untouched, OR change explicitly reviewed

# Vercel-shipping checklist

- [ ] No new Rust function added without a `[[bin]]` entry in root `Cargo.toml`
- [ ] No new env var added without updating `.env.example`
- [ ] If touching `vercel.json`: cron schedules and `maxDuration` reviewed
- [ ] Cold-start budget preserved (binaries < 10 MB; no new heavy deps)
