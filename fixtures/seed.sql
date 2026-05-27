-- Local-development seed: chain rows (idempotent with migration 0002) plus
-- 6 fake agents so /api/v1/agents and the dashboard are non-empty before W1
-- registry-scraper has caught up. Production callers never run this file —
-- the scraper writes real rows.

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

-- 4 Base mainnet agents + 2 Base Sepolia. Owners and wallets are random
-- 20-byte hex. agent_uri values 404 — that's the prober's documented
-- "down" signal, not an error.
INSERT INTO agents
  (chain_id, agent_id, owner, agent_uri, agent_wallet, registered_at, updated_at)
VALUES
  (8453,   1, decode('c0ffee0000000000000000000000000000000001', 'hex'),
              'https://example.com/.well-known/agent-card.json',
              decode('a110ce0000000000000000000000000000000001', 'hex'),
              '2026-02-01T00:00:00Z', '2026-05-20T12:00:00Z'),
  (8453,   2, decode('c0ffee0000000000000000000000000000000002', 'hex'),
              'https://demo.eth-tools.dev/agent-2.json',
              decode('a110ce0000000000000000000000000000000002', 'hex'),
              '2026-02-05T00:00:00Z', '2026-05-21T12:00:00Z'),
  (8453,   3, decode('c0ffee0000000000000000000000000000000003', 'hex'),
              'ipfs://bafyreidemoagent3manifest',
              NULL,
              '2026-03-01T00:00:00Z', '2026-05-22T12:00:00Z'),
  (8453,  42, decode('c0ffee000000000000000000000000000000002a', 'hex'),
              'https://wallet-risk.example/agent.json',
              decode('a110ce000000000000000000000000000000002a', 'hex'),
              '2026-03-15T00:00:00Z', '2026-05-26T12:00:00Z'),
  (84532,  1, decode('5e110e0000000000000000000000000000000001', 'hex'),
              'https://demo.eth-tools.dev/sepolia/agent-1.json',
              NULL,
              '2026-02-10T00:00:00Z', '2026-05-23T12:00:00Z'),
  (84532,  2, decode('5e110e0000000000000000000000000000000002', 'hex'),
              'https://demo.eth-tools.dev/sepolia/agent-2.json',
              decode('a110ce0000000000000000000000000000000003', 'hex'),
              '2026-02-12T00:00:00Z', '2026-05-24T12:00:00Z')
ON CONFLICT (chain_id, agent_id) DO NOTHING;
