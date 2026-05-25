ALTER TABLE pools
    ADD COLUMN IF NOT EXISTS token0          VARCHAR(42),
    ADD COLUMN IF NOT EXISTS token1          VARCHAR(42),
    ADD COLUMN IF NOT EXISTS token0_decimals SMALLINT,
    ADD COLUMN IF NOT EXISTS token1_decimals SMALLINT;

-- PathFinder will query by token address to find all pools containing a token
CREATE INDEX IF NOT EXISTS idx_pools_token0 ON pools (token0);
CREATE INDEX IF NOT EXISTS idx_pools_token1 ON pools (token1);
