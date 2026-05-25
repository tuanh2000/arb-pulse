-- Store the raw per-source prices so a token's effective price_usd is auditable.
-- price_usd remains the effective (highest-priority) price; these hold the raw
-- subgraph inputs that the PulseX/PHUX reconciliation compares.
ALTER TABLE token_price
    ADD COLUMN IF NOT EXISTS price_pulsex DOUBLE PRECISION,
    ADD COLUMN IF NOT EXISTS price_phux   DOUBLE PRECISION;
