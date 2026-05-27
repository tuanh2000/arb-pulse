-- Backfill token_metadata stub rows for every token already referenced in pools.
-- Ensures all known tokens appear in the /api/tokens listing even before the
-- metadata worker resolves symbol/name/decimals. Safe to re-run.
INSERT INTO token_metadata (token_address)
SELECT DISTINCT lower(token0) FROM pools WHERE token0 IS NOT NULL
UNION
SELECT DISTINCT lower(token1) FROM pools WHERE token1 IS NOT NULL
ON CONFLICT (token_address) DO NOTHING;
