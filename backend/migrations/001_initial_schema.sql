-- Enable vector similarity search (for AI dedup - optional, can add later)
-- CREATE EXTENSION IF NOT EXISTS vector;

-- Markets (mirrors on-chain state + stores off-chain metadata)
CREATE TABLE IF NOT EXISTS markets (
    id                SERIAL PRIMARY KEY,
    market_id         INTEGER NOT NULL UNIQUE,
    question          TEXT NOT NULL,
    question_hash     BYTEA NOT NULL,
    category          VARCHAR(50) NOT NULL DEFAULT 'general',
    creator_address   VARCHAR(42) NOT NULL,
    yes_token_address VARCHAR(42),
    no_token_address  VARCHAR(42),
    resolution_date   TIMESTAMPTZ NOT NULL,
    status            VARCHAR(20) NOT NULL DEFAULT 'open',
    outcome           VARCHAR(10),
    created_at        TIMESTAMPTZ DEFAULT NOW(),
    resolved_at       TIMESTAMPTZ,
    tx_hash           VARCHAR(66)
);

CREATE INDEX IF NOT EXISTS idx_markets_status ON markets(status);
CREATE INDEX IF NOT EXISTS idx_markets_category ON markets(category);
CREATE INDEX IF NOT EXISTS idx_markets_market_id ON markets(market_id);

-- Orders (signed orders awaiting on-chain fill)
CREATE TABLE IF NOT EXISTS orders (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    user_address    VARCHAR(42) NOT NULL,
    token           VARCHAR(3) NOT NULL,
    shares          BIGINT NOT NULL,
    cost            BIGINT NOT NULL,
    price           INTEGER NOT NULL,
    nonce           BIGINT NOT NULL,
    signature       BYTEA NOT NULL,
    status          VARCHAR(15) NOT NULL DEFAULT 'pending',
    tx_hash         VARCHAR(66),
    created_at      TIMESTAMPTZ DEFAULT NOW(),
    filled_at       TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_orders_market ON orders(market_id, status);
CREATE INDEX IF NOT EXISTS idx_orders_user ON orders(user_address, status);
CREATE INDEX IF NOT EXISTS idx_orders_market_user ON orders(market_id, user_address);

-- Trades (from OrderFilled events; watcher populates)
CREATE TABLE IF NOT EXISTS trades (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    buyer_address   VARCHAR(42) NOT NULL,
    token           VARCHAR(3) NOT NULL,
    shares          BIGINT NOT NULL,
    cost            BIGINT NOT NULL,
    tx_hash         VARCHAR(66) NOT NULL,
    block_number    BIGINT,
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_trades_market ON trades(market_id);
CREATE INDEX IF NOT EXISTS idx_trades_buyer ON trades(buyer_address);
CREATE INDEX IF NOT EXISTS idx_trades_tx ON trades(tx_hash);

-- Price history (for charts and orderbook-derived probability)
CREATE TABLE IF NOT EXISTS price_snapshots (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    yes_price       INTEGER NOT NULL,
    no_price        INTEGER NOT NULL,
    volume_24h      BIGINT NOT NULL DEFAULT 0,
    timestamp       TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_price_snapshots_market ON price_snapshots(market_id, timestamp);

-- Orderbook levels (aggregated pending orders for probability calculation)
CREATE TABLE IF NOT EXISTS orderbook_levels (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    side            VARCHAR(3) NOT NULL,
    price           INTEGER NOT NULL,
    total_shares    BIGINT NOT NULL DEFAULT 0,
    order_count     INTEGER NOT NULL DEFAULT 0,
    updated_at      TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(market_id, side, price)
);

CREATE INDEX IF NOT EXISTS idx_orderbook_market ON orderbook_levels(market_id);

-- Action mapper: maps user actions to required blockchain operations
CREATE TABLE IF NOT EXISTS action_mapper (
    id              SERIAL PRIMARY KEY,
    action_type     VARCHAR(50) NOT NULL,
    user_address    VARCHAR(42) NOT NULL,
    market_id       INTEGER,
    order_id        INTEGER REFERENCES orders(id),
    required_tx     VARCHAR(50) NOT NULL,
    payload         JSONB,
    status          VARCHAR(20) NOT NULL DEFAULT 'pending',
    tx_hash         VARCHAR(66),
    created_at      TIMESTAMPTZ DEFAULT NOW(),
    completed_at    TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_action_mapper_user ON action_mapper(user_address, status);
CREATE INDEX IF NOT EXISTS idx_action_mapper_type ON action_mapper(action_type, status);

-- Watcher sync state
CREATE TABLE IF NOT EXISTS watcher_cursor (
    id              SERIAL PRIMARY KEY,
    chain_id        INTEGER NOT NULL UNIQUE,
    contract_address VARCHAR(42) NOT NULL,
    last_block      BIGINT NOT NULL DEFAULT 0,
    updated_at      TIMESTAMPTZ DEFAULT NOW()
);
