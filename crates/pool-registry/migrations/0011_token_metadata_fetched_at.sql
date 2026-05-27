ALTER TABLE token_metadata
    ADD COLUMN IF NOT EXISTS metadata_fetched_at TIMESTAMPTZ;
