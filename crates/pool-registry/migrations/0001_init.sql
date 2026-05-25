CREATE TABLE IF NOT EXISTS pools (
    pool_address VARCHAR(42) PRIMARY KEY,
    protocol     VARCHAR(64) NOT NULL,
    tvl          DOUBLE PRECISION,          -- NULL = price unknown
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_pools_updated_at ON pools (updated_at ASC);
CREATE INDEX IF NOT EXISTS idx_pools_tvl        ON pools (tvl);
