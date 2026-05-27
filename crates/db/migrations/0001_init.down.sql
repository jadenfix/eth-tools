-- Roll back 0001_init.sql. Reverse dependency order.

DROP TABLE IF EXISTS alerts;
DROP TABLE IF EXISTS denials;
DROP TABLE IF EXISTS wallet_txs;
DROP TABLE IF EXISTS worker_runs;
DROP TABLE IF EXISTS idempotency_keys;
DROP TABLE IF EXISTS api_keys;
DROP TABLE IF EXISTS cursors;
DROP TABLE IF EXISTS trust_scores;
DROP TABLE IF EXISTS validations;
DROP TABLE IF EXISTS feedback;
DROP TABLE IF EXISTS endpoint_probes;
DROP TABLE IF EXISTS manifests;
DROP TABLE IF EXISTS agents_history;
DROP TABLE IF EXISTS agents;
DROP TABLE IF EXISTS chains;
