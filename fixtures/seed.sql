-- Bootstrap seed for local dev / preview deployments.
-- Real Base Sepolia test agents arrive when we backfill.

INSERT INTO chains (chain_id, name, identity_registry, reputation_registry, validation_registry, rpc_url, is_testnet)
VALUES
  (8453,  'base',         decode('8004A169FB4a3325136EB29fA0ceB6D2e539a432', 'hex'),
                          decode('8004BAa17C55a88189AE136b182e5fdA19dE9b63', 'hex'),
                          decode('8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58', 'hex'),
                          'https://mainnet.base.org', false),
  (84532, 'base-sepolia', decode('8004A169FB4a3325136EB29fA0ceB6D2e539a432', 'hex'),
                          decode('8004BAa17C55a88189AE136b182e5fdA19dE9b63', 'hex'),
                          decode('8004Cc8439f36fd5F9F049D9fF86523Df6dAAB58', 'hex'),
                          'https://sepolia.base.org', true)
ON CONFLICT (chain_id) DO NOTHING;
