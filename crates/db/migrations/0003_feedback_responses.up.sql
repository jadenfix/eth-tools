-- 0003_feedback_responses.sql — store ResponseAppended events from the
-- Reputation registry (plan §3 W4 row).
--
-- ResponseAppended(agentId, clientAddress, feedbackIndex, responder,
--                  responseURI, responseHash) — emitted from
-- ReputationRegistryUpgradeable.appendResponse(). Anyone may append a
-- response to any feedback (responder = msg.sender). A single feedback can
-- have many responders, each may append multiple times.
--
-- Storage shape rationale: we keep a row per (responder, feedback) tuple
-- with a row counter to disambiguate repeat appends. Reusing the `feedback`
-- table with a synthetic feedback_index would collide on the existing
-- (chain_id, agent_id, client_address, feedback_index) PK — see W4 PR
-- description for the full decision log.

CREATE TABLE feedback_responses (
  chain_id        BIGINT NOT NULL,
  agent_id        NUMERIC(78, 0) NOT NULL,
  client_address  BYTEA NOT NULL,
  feedback_index  BIGINT NOT NULL,
  responder       BYTEA NOT NULL,
  -- Disambiguator for multiple appends from the same responder. Sourced
  -- from the log's `(block_number, log_index)` so on-chain ordering is
  -- preserved and replay is idempotent via the PK.
  block_number    BIGINT NOT NULL,
  log_index       INT    NOT NULL,
  response_uri    TEXT,
  response_hash   BYTEA,
  tx_hash         BYTEA NOT NULL,
  appended_at     TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (chain_id, agent_id, client_address, feedback_index, responder, block_number, log_index)
);

CREATE INDEX idx_feedback_responses_agent
  ON feedback_responses (chain_id, agent_id, appended_at DESC);
CREATE INDEX idx_feedback_responses_feedback
  ON feedback_responses (chain_id, agent_id, client_address, feedback_index);
