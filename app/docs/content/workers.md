# Workers

Eight cron-driven workers populate the database from on-chain state and
agent metadata. Cadences are configured per worker; the SLO table on
`/dashboard/workers` flags any run that's `STALE` (>1.5× cadence) or
`CRIT` (>3× cadence).

| Worker                    | Cadence | What it does                                    |
| ------------------------- | ------- | ----------------------------------------------- |
| `registry_scraper`        | 60s     | Tail the IdentityRegistry for `Registered`/`Updated`/`Transferred`. |
| `manifest_fetcher`        | 5m      | Fetch new `agent_uri` payloads; validate + hash. |
| `endpoint_prober`         | 5m      | Probe each declared endpoint; record latency.   |
| `feedback_aggregator`     | 5m      | Roll up ReputationRegistry feedback events.     |
| `validation_aggregator`   | 5m      | Roll up ValidationRegistry responses.           |
| `trust_scorer`            | 15m     | Recompute trust scores for changed agents.      |
| `wallet_balance_keeper`   | 60s     | Refresh agent EOA USDC balance into `wallet_txs`. |
| `wallet_rotation_watcher` | 1h      | Catch agent-wallet rotations from on-chain events. |

Every run writes one row to `worker_runs` with `rows_in`, `rows_out`,
and `error`. Failures bubble into the `alerts` table once severity
thresholds are crossed.
