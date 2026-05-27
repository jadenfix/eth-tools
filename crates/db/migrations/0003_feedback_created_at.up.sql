-- 0003_feedback_created_at — add a wall-clock timestamp to feedback rows so
-- W7 (trust_score_recompute) can window by "last 90 days" without having to
-- approximate via block_number.
--
-- The on-chain event doesn't carry a timestamp (only a block_number); W4
-- (reputation_aggregator, lands later) is responsible for stamping this
-- column with `block.timestamp` as it ingests. For now, DEFAULT NOW() so
-- any future rows have a real value and W7's WHERE clause is correct.

ALTER TABLE feedback
  ADD COLUMN IF NOT EXISTS created_at TIMESTAMPTZ NOT NULL DEFAULT NOW();

CREATE INDEX IF NOT EXISTS idx_feedback_created_at
  ON feedback (chain_id, agent_id, created_at DESC);
