-- Meme-coin token flag. A token flagged here causes every pool containing it
-- to be excluded from /pools, so the listener never tracks it and it can never
-- enter a cycle or reach the broadcaster. Set by the config meme denylist at
-- startup and the keyword-based meme screener worker.
ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS is_meme BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS meme_screened_at TIMESTAMPTZ;

-- Partial index: the /pools exclusion only ever looks up flagged tokens.
CREATE INDEX IF NOT EXISTS idx_token_metadata_is_meme
    ON token_metadata (token_address) WHERE is_meme;

-- Pool-level denormalized flag: true when either token0 or token1 is a meme
-- coin. Maintained by mark_token_meme (per-token write) and sync_pool_meme_flags
-- (periodic full sync after each screener batch). Used by /pools to filter out
-- meme pools in a single column check instead of a correlated subquery.
ALTER TABLE pools
    ADD COLUMN IF NOT EXISTS has_meme_token BOOLEAN NOT NULL DEFAULT FALSE;

CREATE INDEX IF NOT EXISTS idx_pools_has_meme_token
    ON pools (pool_address) WHERE has_meme_token;
