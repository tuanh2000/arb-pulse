-- Token metadata fetched on-chain (symbol / name / decimals).
-- Distinct from token_price: this holds identity, not valuation.
CREATE TABLE IF NOT EXISTS token_metadata (
    token_address VARCHAR(42) PRIMARY KEY,
    symbol        TEXT,
    name          TEXT,
    decimals      SMALLINT,
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_token_metadata_updated_at ON token_metadata (updated_at);
