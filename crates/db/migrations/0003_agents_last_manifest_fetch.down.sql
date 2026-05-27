-- Revert 0003_agents_last_manifest_fetch.
DROP INDEX IF EXISTS idx_agents_manifest_due;
ALTER TABLE agents
  DROP COLUMN IF EXISTS last_manifest_fetch;
