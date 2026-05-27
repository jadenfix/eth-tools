-- 0002_chains_seed down — remove the two seed rows. Will fail if `agents`
-- still has rows referencing them; callers must clean up children first.
DELETE FROM chains WHERE chain_id IN (8453, 84532);
