-- 0001_init.sql — canonical schema from plan §7.
--
-- All addresses + hashes are stored as BYTEA (20 / 32 bytes) — half the size
-- of hex text and faster to index.
-- `agent_id` is NUMERIC(78, 0) to hold a full uint256.

CREATE TABLE chains (
  chain_id            BIGINT PRIMARY KEY,
  name                TEXT NOT NULL UNIQUE,
  identity_registry   BYTEA NOT NULL,
  reputation_registry BYTEA NOT NULL,
  validation_registry BYTEA NOT NULL,
  rpc_url             TEXT NOT NULL,
  is_testnet          BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE agents (
  chain_id       BIGINT NOT NULL REFERENCES chains(chain_id),
  agent_id       NUMERIC(78, 0) NOT NULL,
  owner          BYTEA NOT NULL,
  agent_uri      TEXT,
  agent_wallet   BYTEA,                     -- nullable; cleared on Transfer (spec gotcha #2)
  registered_at  TIMESTAMPTZ NOT NULL,
  updated_at     TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (chain_id, agent_id)
);
CREATE INDEX idx_agents_owner   ON agents (owner);
CREATE INDEX idx_agents_wallet  ON agents (agent_wallet) WHERE agent_wallet IS NOT NULL;
CREATE INDEX idx_agents_updated ON agents (updated_at DESC);

CREATE TABLE agents_history (
  id           BIGSERIAL PRIMARY KEY,
  chain_id     BIGINT NOT NULL,
  agent_id     NUMERIC(78, 0) NOT NULL,
  changed_at   TIMESTAMPTZ NOT NULL,
  change_kind  TEXT NOT NULL CHECK (change_kind IN ('uri','wallet','owner','metadata')),
  before_value JSONB,
  after_value  JSONB,
  tx_hash      BYTEA NOT NULL,
  log_index    INT NOT NULL
);
CREATE INDEX idx_history_agent ON agents_history (chain_id, agent_id, changed_at DESC);

CREATE TABLE manifests (
  chain_id          BIGINT NOT NULL,
  agent_id          NUMERIC(78, 0) NOT NULL,
  fetched_at        TIMESTAMPTZ NOT NULL,
  source_uri        TEXT NOT NULL,
  raw_bytes_sha256  BYTEA NOT NULL,
  raw_keccak256     BYTEA NOT NULL,
  parsed            JSONB,
  validation_status TEXT NOT NULL CHECK (validation_status IN ('valid','invalid','unreachable')),
  validation_errors JSONB,
  blob_url          TEXT,
  PRIMARY KEY (chain_id, agent_id, raw_bytes_sha256)
);
CREATE INDEX idx_manifests_latest ON manifests (chain_id, agent_id, fetched_at DESC);

CREATE TABLE endpoint_probes (
  id           BIGSERIAL PRIMARY KEY,
  chain_id     BIGINT NOT NULL,
  agent_id     NUMERIC(78, 0) NOT NULL,
  service_name TEXT NOT NULL,
  endpoint     TEXT NOT NULL,
  probed_at    TIMESTAMPTZ NOT NULL,
  status_code  INT,
  latency_ms   INT,
  ok           BOOLEAN NOT NULL,
  error        TEXT
);
CREATE INDEX idx_probes_agent ON endpoint_probes (chain_id, agent_id, probed_at DESC);

CREATE TABLE feedback (
  chain_id        BIGINT NOT NULL,
  agent_id        NUMERIC(78, 0) NOT NULL,
  client_address  BYTEA NOT NULL,
  feedback_index  BIGINT NOT NULL,
  value           NUMERIC(40, 0) NOT NULL,
  value_decimals  SMALLINT NOT NULL CHECK (value_decimals BETWEEN 0 AND 18),
  tag1            TEXT,
  tag2            TEXT,
  endpoint        TEXT,
  feedback_uri    TEXT,
  feedback_hash   BYTEA,
  is_revoked      BOOLEAN NOT NULL DEFAULT FALSE,
  tx_hash         BYTEA NOT NULL,
  block_number    BIGINT NOT NULL,
  PRIMARY KEY (chain_id, agent_id, client_address, feedback_index)
);
CREATE INDEX idx_feedback_agent ON feedback (chain_id, agent_id, tag1);

CREATE TABLE validations (
  chain_id       BIGINT NOT NULL,
  request_hash   BYTEA NOT NULL,
  validator_addr BYTEA NOT NULL,
  agent_id       NUMERIC(78, 0) NOT NULL,
  request_uri    TEXT NOT NULL,
  response       SMALLINT CHECK (response BETWEEN 0 AND 100),
  response_uri   TEXT,
  response_hash  BYTEA,
  tag            TEXT,
  last_update    TIMESTAMPTZ NOT NULL,
  PRIMARY KEY (chain_id, request_hash)
);
CREATE INDEX idx_validations_agent ON validations (chain_id, agent_id, last_update DESC);

CREATE TABLE trust_scores (
  chain_id       BIGINT NOT NULL,
  agent_id       NUMERIC(78, 0) NOT NULL,
  scorer_version TEXT NOT NULL,
  computed_at    TIMESTAMPTZ NOT NULL,
  score          INT NOT NULL CHECK (score BETWEEN 0 AND 100),
  components     JSONB NOT NULL,
  PRIMARY KEY (chain_id, agent_id, scorer_version)
);

CREATE TABLE cursors (
  cursor_key     TEXT PRIMARY KEY,
  last_block     BIGINT NOT NULL,
  last_log_index INT NOT NULL,
  updated_at     TIMESTAMPTZ NOT NULL
);

CREATE TABLE api_keys (
  id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
  github_user_id BIGINT NOT NULL,
  github_login   TEXT NOT NULL,
  key_prefix     TEXT NOT NULL UNIQUE,
  key_hash       TEXT NOT NULL,
  name           TEXT NOT NULL,
  scopes         TEXT[] NOT NULL DEFAULT ARRAY['read'],
  created_at     TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  last_used_at   TIMESTAMPTZ,
  revoked_at     TIMESTAMPTZ
);
CREATE INDEX idx_keys_owner ON api_keys (github_user_id);

CREATE TABLE idempotency_keys (
  key_hash        BYTEA PRIMARY KEY,
  api_key_id      UUID REFERENCES api_keys(id),
  request_path    TEXT NOT NULL,
  response_status INT NOT NULL,
  response_body   JSONB NOT NULL,
  created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  expires_at      TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_idem_expires ON idempotency_keys (expires_at);

CREATE TABLE worker_runs (
  id          BIGSERIAL PRIMARY KEY,
  worker_name TEXT NOT NULL,
  vercel_env  TEXT NOT NULL,
  started_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  finished_at TIMESTAMPTZ,
  rows_in     INT,
  rows_out    INT,
  ok          BOOLEAN,
  error       TEXT,
  dryrun      BOOLEAN NOT NULL DEFAULT FALSE
);
CREATE INDEX idx_worker_runs ON worker_runs (worker_name, started_at DESC);

CREATE TABLE wallet_txs (
  id            BIGSERIAL PRIMARY KEY,
  chain_id      BIGINT NOT NULL,
  nonce         BIGINT,
  to_addr       BYTEA NOT NULL,
  selector      BYTEA NOT NULL,
  value_wei     NUMERIC(40, 0) NOT NULL DEFAULT 0,
  gas_used      BIGINT,
  fee_usdc      NUMERIC(20, 6),
  tx_hash       BYTEA UNIQUE,
  status        TEXT NOT NULL CHECK (status IN ('queued','submitted','confirmed','reverted','rejected')),
  reject_reason TEXT,
  api_key_id    UUID REFERENCES api_keys(id),
  created_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
CREATE INDEX idx_wtx_status ON wallet_txs (status, created_at);
CREATE INDEX idx_wtx_chain  ON wallet_txs (chain_id, nonce);

CREATE TABLE denials (
  id            BIGSERIAL PRIMARY KEY,
  occurred_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  code          TEXT NOT NULL,
  evaluator     TEXT NOT NULL,
  api_key_id    UUID REFERENCES api_keys(id),
  request_path  TEXT NOT NULL,
  details       JSONB
);
CREATE INDEX idx_denials_code ON denials (code, occurred_at DESC);

CREATE TABLE alerts (
  id           BIGSERIAL PRIMARY KEY,
  raised_at    TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  severity     TEXT NOT NULL CHECK (severity IN ('info','warn','crit')),
  source       TEXT NOT NULL,
  body         JSONB NOT NULL,
  acknowledged BOOLEAN NOT NULL DEFAULT FALSE,
  ack_by       TEXT,
  ack_at       TIMESTAMPTZ
);
CREATE INDEX idx_alerts_open ON alerts (severity, raised_at DESC) WHERE acknowledged = FALSE;
