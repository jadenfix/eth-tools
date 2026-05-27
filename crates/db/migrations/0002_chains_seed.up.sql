-- 0002_chains_seed — bootstrap the `chains` table with Base mainnet + Base
-- Sepolia rows. Universal ERC-8004 registry addresses are identical across
-- both chains (CREATE2 + same salts).
--
-- Idempotent: ON CONFLICT keeps the row in sync with `crates/core::chains`
-- so re-applying this migration (preview replays, ad-hoc reruns) is safe.
-- `agents.chain_id REFERENCES chains(chain_id)` means the agents read-path
-- needs these rows present or every insert FKs.

INSERT INTO chains
  (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
VALUES
  (8453, 'base',
    '\x8004A169FB4a3325136EB29fA0ceB6D2e539a432'::bytea,
    '\x8004BAa17C55a88189AE136b182e5fdA19dE9b63'::bytea,
    '\x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58'::bytea,
    'https://mainnet.base.org',
    FALSE),
  (84532, 'base-sepolia',
    '\x8004A169FB4a3325136EB29fA0ceB6D2e539a432'::bytea,
    '\x8004BAa17C55a88189AE136b182e5fdA19dE9b63'::bytea,
    '\x8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58'::bytea,
    'https://sepolia.base.org',
    TRUE)
ON CONFLICT (chain_id) DO UPDATE SET
  name                = EXCLUDED.name,
  identity_registry   = EXCLUDED.identity_registry,
  reputation_registry = EXCLUDED.reputation_registry,
  validation_registry = EXCLUDED.validation_registry,
  is_testnet          = EXCLUDED.is_testnet;
