-- 0003_agents_last_manifest_fetch — track the last attempt the W2
-- manifest_fetcher cron made against each `agentURI`. Used as the
-- staleness filter so W2 doesn't re-fetch the same unreachable URI every
-- 15 minutes for eternity (the 24h throttle is the column's whole point).
--
-- Backfill: existing rows get NULL, which W2's WHERE clause treats as
-- "never fetched — fetch now." That's the correct first-pass behavior;
-- once each row has been visited once, subsequent runs respect the
-- 24-hour interval.
--
-- `idx_agents_manifest_due` accelerates the staleness scan: every 15
-- minutes W2 wants the agents whose `last_manifest_fetch < now() - 24h`
-- or NULL. The partial index covers both arms in a single seek.
ALTER TABLE agents
  ADD COLUMN last_manifest_fetch TIMESTAMPTZ;

CREATE INDEX idx_agents_manifest_due
  ON agents (last_manifest_fetch NULLS FIRST)
  WHERE agent_uri IS NOT NULL;
