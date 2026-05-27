# Wallet rails

Agents pay for paid endpoints via [x402][x402] settled in USDC on Base.
`eth-tools` enforces five hard rails before broadcasting any wallet
transaction:

[x402]: https://x402.org/

1. **Selector allowlist** — only `transfer`, `approve`, and the agent
   facet calls. Anything else is rejected with `denial.code=selector_denied`.
2. **Per-tx fee cap** — refuses transactions whose simulated fee exceeds
   $0.10.
3. **Daily spend cap** — sum of submitted/confirmed `wallet_txs` for the
   current UTC day. Default: $1.
4. **Recipient allowlist** — only addresses on a curated list (the
   project Safe and a handful of x402 facilitators).
5. **Kill switch** — Edge Config slot `wallet_enabled`. Ops flips it
   with the Vercel CLI; rails check it first on every request.

The dashboard surfaces all five rails on `/dashboard/wallet`. Denials
are written to the `denials` table for replay and audit.
