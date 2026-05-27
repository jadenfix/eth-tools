-- 0003_wallet_nonces.sql — Postgres-backed nonce allocator (plan §10 + phase 6.2).
--
-- Vercel isolates have no shared in-process state; calling
-- `eth_getTransactionCount(signer, "pending")` per tx is racy when two
-- isolates dispatch concurrently. Instead we store a monotonic counter in
-- Postgres and dispense the next value inside a `BEGIN`+`SELECT ... FOR
-- UPDATE`+`UPDATE`+`COMMIT` transaction so two concurrent dispenses always
-- produce distinct sequential nonces.
--
-- One row per (chain_id, signer_address). `next_nonce` holds the value that
-- will be handed out by the NEXT call; on first use the bootstrap path
-- reads `eth_getTransactionCount(signer, "pending")` and INSERTs.

CREATE TABLE wallet_nonces (
  chain_id        BIGINT NOT NULL,
  signer_address  BYTEA  NOT NULL,
  next_nonce      BIGINT NOT NULL,
  updated_at      TIMESTAMPTZ NOT NULL DEFAULT NOW(),
  PRIMARY KEY (chain_id, signer_address)
);
