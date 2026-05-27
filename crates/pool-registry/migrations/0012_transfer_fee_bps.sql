ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS transfer_fee_bps SMALLINT;
