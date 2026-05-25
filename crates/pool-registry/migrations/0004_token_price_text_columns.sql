-- Some token names exceed VARCHAR(128) and some symbols exceed VARCHAR(64).
-- Use TEXT (unlimited) to avoid truncation errors.
ALTER TABLE token_price
    ALTER COLUMN symbol TYPE TEXT,
    ALTER COLUMN name   TYPE TEXT;
