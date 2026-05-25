CREATE TABLE IF NOT EXISTS token_price (
    token_address  VARCHAR(42)  PRIMARY KEY,
    symbol         VARCHAR(64),
    name           VARCHAR(128),
    price_usd      DOUBLE PRECISION NOT NULL,
    source         VARCHAR(32)  NOT NULL,  -- 'pulsex_subgraph' | 'hardcoded' | 'dexscreener'
    updated_at     TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_token_price_updated_at ON token_price (updated_at);
