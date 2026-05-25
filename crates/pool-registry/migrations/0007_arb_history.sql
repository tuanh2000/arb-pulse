-- Arbitrage history: opportunities found by the finder and transactions sent by the broadcaster.
-- Component event log used by the orchestrator to record starts, crashes, and restarts.

CREATE TABLE IF NOT EXISTS opportunities (
    id               BIGSERIAL    PRIMARY KEY,
    discovered_at    TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    block_number     BIGINT,
    token_in         VARCHAR(42)  NOT NULL,
    amount_in_raw    TEXT         NOT NULL,
    expected_out_raw TEXT         NOT NULL,
    profit_raw       TEXT         NOT NULL,
    profit_human     DOUBLE PRECISION NOT NULL,
    net_profit_human DOUBLE PRECISION NOT NULL,
    hops             JSONB        NOT NULL
);

CREATE TABLE IF NOT EXISTS arb_transactions (
    id              BIGSERIAL    PRIMARY KEY,
    opportunity_id  BIGINT       REFERENCES opportunities(id) ON DELETE SET NULL,
    submitted_at    TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    tx_hash         VARCHAR(66),
    status          VARCHAR(20)  NOT NULL DEFAULT 'pending',
    block_number    BIGINT,
    gas_used        BIGINT,
    error_msg       TEXT,
    updated_at      TIMESTAMPTZ  NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS component_events (
    id           BIGSERIAL    PRIMARY KEY,
    occurred_at  TIMESTAMPTZ  NOT NULL DEFAULT NOW(),
    component    VARCHAR(50)  NOT NULL,
    event_type   VARCHAR(30)  NOT NULL,
    pid          INT,
    detail       TEXT
);

CREATE INDEX IF NOT EXISTS idx_opps_block        ON opportunities(block_number DESC);
CREATE INDEX IF NOT EXISTS idx_arb_status        ON arb_transactions(status);
CREATE INDEX IF NOT EXISTS idx_arb_opp           ON arb_transactions(opportunity_id);
CREATE INDEX IF NOT EXISTS idx_comp_events_comp  ON component_events(component, occurred_at DESC);
