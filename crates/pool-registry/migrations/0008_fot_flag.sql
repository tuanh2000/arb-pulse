-- Fee-on-transfer / gas-heavy token screening flag. A token flagged here causes
-- every pool containing it to be excluded from /pools, so the listener never
-- tracks it and it can never enter a cycle or reach the broadcaster. The flag is
-- set by the config denylist (at startup) and the FoT screener worker.
ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS is_fot BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS screened_at TIMESTAMPTZ;

-- Partial index: the /pools exclusion only ever looks up flagged tokens.
CREATE INDEX IF NOT EXISTS idx_token_metadata_is_fot
    ON token_metadata (token_address) WHERE is_fot;
