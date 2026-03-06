# Backend Spec — Prediction Market Platform

## Overview

The backend serves as the middleware between the frontend, smart contracts, and CRE workflow. It handles three core responsibilities:

1. **Quest Management** — create markets with AI-powered deduplication
2. **Order Matching** — off-chain orderbook that settles trades on-chain
3. **Data Serving** — market data, prices, portfolios, and activity feeds

---

## Tech Stack

```
Runtime:       Bun
Framework:     Hono (lightweight, fast, TypeScript-native)
Database:      PostgreSQL + pgvector extension
AI:            Gemini 2.5 Flash Lite (free tier — dedup + embeddings)
Blockchain:    viem (contract reads/writes)
Auth:          Wallet signature (EIP-4361 / SIWE)
```

Why Hono over Express: faster startup, native TypeScript, runs on Bun, smaller bundle. For a hackathon it's the fastest path.

---

## Database Schema

```sql
-- Enable vector similarity search
CREATE EXTENSION IF NOT EXISTS vector;

-- Markets (mirrors on-chain state + stores off-chain metadata)
CREATE TABLE markets (
    id                SERIAL PRIMARY KEY,
    market_id         INTEGER NOT NULL UNIQUE,          -- on-chain marketId
    question          TEXT NOT NULL,                      -- full question text
    question_hash     BYTEA NOT NULL,                    -- keccak256, matches on-chain
    embedding         VECTOR(768) NOT NULL,              -- for similarity search
    category          VARCHAR(50) NOT NULL DEFAULT 'general',
    creator_address   VARCHAR(42) NOT NULL,              -- ETH address
    yes_token_address VARCHAR(42),                       -- deployed OutcomeToken
    no_token_address  VARCHAR(42),                       -- deployed OutcomeToken
    resolution_date   TIMESTAMPTZ NOT NULL,
    status            VARCHAR(20) NOT NULL DEFAULT 'open',  -- open, resolved, cancelled
    outcome           VARCHAR(10),                       -- null, YES, NO
    created_at        TIMESTAMPTZ DEFAULT NOW(),
    resolved_at       TIMESTAMPTZ,
    tx_hash           VARCHAR(66)                        -- creation tx
);

CREATE INDEX idx_markets_embedding ON markets
    USING ivfflat (embedding vector_cosine_ops)
    WITH (lists = 100);
CREATE INDEX idx_markets_status ON markets(status);
CREATE INDEX idx_markets_category ON markets(category);

-- Orders (the off-chain orderbook)
CREATE TABLE orders (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    user_address    VARCHAR(42) NOT NULL,
    side            VARCHAR(4) NOT NULL,                -- 'buy' or 'sell'
    token           VARCHAR(3) NOT NULL,                -- 'YES' or 'NO'
    amount          BIGINT NOT NULL,                    -- in token units (6 decimals)
    price           INTEGER NOT NULL,                   -- in cents (1-99)
    filled_amount   BIGINT NOT NULL DEFAULT 0,
    status          VARCHAR(10) NOT NULL DEFAULT 'open', -- open, filled, partial, cancelled
    created_at      TIMESTAMPTZ DEFAULT NOW(),
    filled_at       TIMESTAMPTZ
);

CREATE INDEX idx_orders_market ON orders(market_id, status);
CREATE INDEX idx_orders_matching ON orders(market_id, token, side, price, status);

-- Trades (matched order pairs)
CREATE TABLE trades (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    buy_order_id    INTEGER REFERENCES orders(id),
    sell_order_id   INTEGER REFERENCES orders(id),
    buyer_address   VARCHAR(42) NOT NULL,
    seller_address  VARCHAR(42) NOT NULL,
    token           VARCHAR(3) NOT NULL,                -- YES or NO
    amount          BIGINT NOT NULL,
    price           INTEGER NOT NULL,                   -- cents
    tx_hash         VARCHAR(66),                        -- on-chain settlement tx
    created_at      TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_trades_market ON trades(market_id);

-- Price history (for charts)
CREATE TABLE price_snapshots (
    id              SERIAL PRIMARY KEY,
    market_id       INTEGER NOT NULL REFERENCES markets(market_id),
    yes_price       INTEGER NOT NULL,                   -- cents
    volume_24h      BIGINT NOT NULL DEFAULT 0,
    timestamp       TIMESTAMPTZ DEFAULT NOW()
);

CREATE INDEX idx_price_snapshots_market ON price_snapshots(market_id, timestamp);
```

---

## API Endpoints

### Quest Management

#### `POST /api/markets/create`

Create a new prediction market with AI deduplication.

```
Request:
{
    "question": "Will Bitcoin hit $100k by December 31, 2025?",
    "category": "crypto",
    "resolutionDate": "2025-12-31T00:00:00Z",
    "creatorAddress": "0x742d...5f0bEb"
}

Response (success):
{
    "marketId": 0,
    "questionHash": "0xabc123...",
    "yesTokenAddress": "0x...",
    "noTokenAddress": "0x...",
    "txHash": "0x...",
    "status": "open"
}

Response (duplicate detected):
{
    "error": "similar_market_exists",
    "similarMarket": {
        "marketId": 3,
        "question": "Will BTC reach $100,000 before 2026?",
        "similarity": 0.91
    },
    "message": "A similar market already exists. Please modify your question."
}
```

**Deduplication flow:**
```
1. Generate embedding of question text via Gemini embedding API
2. Query pgvector: SELECT * FROM markets WHERE embedding <=> $1 < 0.15
   (cosine distance < 0.15 = similarity > 0.85)
3. If similar market found:
   a. Call Gemini LLM as second check:
      "Are these two prediction market questions asking the same thing?
       Q1: {new_question}
       Q2: {existing_question}
       Answer only YES or NO."
   b. If LLM says YES → reject with similar market info
4. If no duplicate:
   a. Call PredictionMarket.createMarket(questionHash, resolutionTimestamp)
   b. Read MarketCreated event to get token addresses
   c. Store in database with embedding
   d. Return market info
```

#### `GET /api/markets`

List markets with filtering and sorting.

```
Query params:
  ?status=open|resolved|all        (default: open)
  ?category=crypto|politics|sports  (optional)
  ?sort=newest|volume|ending_soon   (default: newest)
  ?limit=20&offset=0

Response:
{
    "markets": [
        {
            "marketId": 0,
            "question": "Will Bitcoin hit $100k by December 31, 2025?",
            "category": "crypto",
            "yesPrice": 72,          // cents — from last trade
            "noPrice": 28,
            "volume24h": 2400000,    // in USDC smallest unit
            "resolutionDate": "2025-12-31T00:00:00Z",
            "status": "open",
            "creatorAddress": "0x742d..."
        }
    ],
    "total": 42
}
```

#### `GET /api/markets/:marketId`

Get full market details.

```
Response:
{
    "marketId": 0,
    "question": "Will Bitcoin hit $100k by December 31, 2025?",
    "category": "crypto",
    "creator": "0x742d...",
    "yesTokenAddress": "0x...",
    "noTokenAddress": "0x...",
    "yesPrice": 72,
    "noPrice": 28,
    "totalVolume": 15000000,
    "totalLiquidity": 5000000,
    "resolutionDate": "2025-12-31T00:00:00Z",
    "status": "open",
    "outcome": null,
    "createdAt": "2025-06-15T10:00:00Z",
    "priceHistory": [
        { "timestamp": "2025-06-15T10:00:00Z", "yesPrice": 50 },
        { "timestamp": "2025-06-15T11:00:00Z", "yesPrice": 55 },
        { "timestamp": "2025-06-15T12:00:00Z", "yesPrice": 72 }
    ],
    "recentTrades": [
        {
            "buyer": "0x3f2...",
            "token": "YES",
            "amount": 500000,
            "price": 72,
            "timestamp": "2025-06-15T12:30:00Z"
        }
    ]
}
```

#### `GET /api/markets/check-duplicate`

Real-time dedup check as user types (debounced from frontend).

```
Query params:
  ?question=Will Bitcoin hit 100k...

Response:
{
    "isDuplicate": true,
    "similarity": 0.91,
    "similarMarket": {
        "marketId": 3,
        "question": "Will BTC reach $100,000 before 2026?"
    }
}
```

---

### Trading (Orderbook)

#### `POST /api/orders`

Place a new order.

```
Request:
{
    "marketId": 0,
    "side": "buy",
    "token": "YES",
    "amount": 1000000,       // 1 USDC worth of tokens
    "price": 72,             // willing to pay 72 cents per YES token
    "userAddress": "0x742d..."
}

Response:
{
    "orderId": 15,
    "status": "filled",       // or "open" if no match
    "filledAmount": 1000000,
    "trade": {                 // only if matched
        "tradeId": 8,
        "counterparty": "0x3f2...",
        "price": 72,
        "txHash": "0x..."     // on-chain settlement
    }
}
```

**Matching logic:**
```
New buy order for YES at 72¢ comes in:

1. Check for matching sell orders:
   SELECT * FROM orders
   WHERE market_id = $1
     AND token = 'YES'
     AND side = 'sell'
     AND price <= 72           -- seller willing to sell at or below 72¢
     AND status = 'open'
   ORDER BY price ASC, created_at ASC
   LIMIT 1

2. If match found (e.g., sell YES at 70¢):
   a. Execute at seller's price (70¢) — price improvement for buyer
   b. On-chain: transfer YES tokens from seller → buyer
   c. On-chain: transfer 70¢ USDC from buyer → seller
   d. Update both orders to 'filled'
   e. Insert into trades table
   f. Snapshot new price

3. If no direct match, check for synthetic match:
   Someone wanting to buy NO at 28¢ is equivalent to someone
   selling YES at 72¢ (because YES + NO = $1)

   If found:
   a. Call PredictionMarket.mintTokens() — mint YES + NO pair for $1
   b. Send YES to the YES buyer, NO to the NO buyer
   c. Combined cost: 72¢ (YES buyer) + 28¢ (NO buyer) = $1

4. If no match at all:
   a. Store as open order
   b. Return status: "open"
```

#### `GET /api/orders/:marketId`

Get current orderbook for a market.

```
Response:
{
    "marketId": 0,
    "bids": [                          // buy orders, sorted price descending
        { "price": 72, "amount": 5000000, "orders": 3 },
        { "price": 70, "amount": 2000000, "orders": 1 },
        { "price": 68, "amount": 8000000, "orders": 5 }
    ],
    "asks": [                          // sell orders, sorted price ascending
        { "price": 74, "amount": 3000000, "orders": 2 },
        { "price": 76, "amount": 1000000, "orders": 1 },
        { "price": 80, "amount": 4000000, "orders": 3 }
    ],
    "lastPrice": 72,
    "spread": 2                        // 74 - 72
}
```

#### `DELETE /api/orders/:orderId`

Cancel an open order.

```
Response:
{
    "orderId": 15,
    "status": "cancelled"
}
```

---

### Portfolio

#### `GET /api/portfolio/:address`

Get user's positions across all markets.

```
Response:
{
    "address": "0x742d...",
    "positions": [
        {
            "marketId": 0,
            "question": "Will Bitcoin hit $100k by December 31, 2025?",
            "yesBalance": 5000000,
            "noBalance": 0,
            "avgBuyPrice": 68,           // cents
            "currentPrice": 72,          // cents
            "unrealizedPnl": 200000,     // in USDC units
            "status": "open"
        }
    ],
    "openOrders": [
        {
            "orderId": 22,
            "marketId": 1,
            "side": "buy",
            "token": "YES",
            "amount": 2000000,
            "price": 55
        }
    ],
    "totalPnl": 350000
}
```

#### `GET /api/portfolio/:address/history`

Trade history for a user.

```
Response:
{
    "trades": [
        {
            "tradeId": 8,
            "marketId": 0,
            "side": "buy",
            "token": "YES",
            "amount": 1000000,
            "price": 72,
            "timestamp": "2025-06-15T12:30:00Z",
            "txHash": "0x..."
        }
    ]
}
```

---

### Resolution

#### `GET /api/resolution/:marketId`

Get resolution details (from CRE workflow output).

```
Response:
{
    "marketId": 0,
    "status": "resolved",
    "outcome": "NO",
    "confidence": 0.7,
    "reasoning": "Based on current market trajectory...",
    "evidenceHash": "00049813dba41648",
    "resolvedAt": "2025-12-31T00:05:00Z",
    "txHash": "0x967cd340..."
}
```

---

## AI Deduplication — Detail

### Embedding Generation

```typescript
async function generateEmbedding(text: string): Promise<number[]> {
    const response = await fetch(
        "https://generativelanguage.googleapis.com/v1beta/models/text-embedding-004:embedContent",
        {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                "x-goog-api-key": GEMINI_API_KEY,
            },
            body: JSON.stringify({
                model: "models/text-embedding-004",
                content: { parts: [{ text }] },
            }),
        }
    );
    const data = await response.json();
    return data.embedding.values; // 768-dim vector
}
```

### Similarity Check

```typescript
async function findSimilarMarket(question: string): Promise<SimilarMarket | null> {
    const embedding = await generateEmbedding(question);

    // pgvector cosine distance: lower = more similar
    // <=> operator returns cosine distance (0 = identical, 2 = opposite)
    const result = await db.query(`
        SELECT market_id, question, (embedding <=> $1::vector) as distance
        FROM markets
        WHERE status = 'open'
        ORDER BY embedding <=> $1::vector
        LIMIT 1
    `, [JSON.stringify(embedding)]);

    if (result.rows.length === 0) return null;

    const similarity = 1 - result.rows[0].distance; // convert distance to similarity
    if (similarity < 0.85) return null;

    // Double-check with LLM
    const llmConfirm = await confirmDuplicateWithLLM(
        question,
        result.rows[0].question
    );

    if (!llmConfirm) return null;

    return {
        marketId: result.rows[0].market_id,
        question: result.rows[0].question,
        similarity: Math.round(similarity * 100),
    };
}
```

### LLM Confirmation

```typescript
async function confirmDuplicateWithLLM(q1: string, q2: string): Promise<boolean> {
    const response = await fetch(
        "https://generativelanguage.googleapis.com/v1beta/models/gemini-2.5-flash-lite:generateContent",
        {
            method: "POST",
            headers: {
                "Content-Type": "application/json",
                "x-goog-api-key": GEMINI_API_KEY,
            },
            body: JSON.stringify({
                contents: [{
                    parts: [{
                        text: `Are these two prediction market questions asking essentially the same thing? Consider if a user betting on Q1 would also be satisfied betting on Q2.

Q1: ${q1}
Q2: ${q2}

Answer only YES or NO.`
                    }]
                }]
            }),
        }
    );
    const data = await response.json();
    const answer = data.candidates[0].content.parts[0].text.trim().toUpperCase();
    return answer.includes("YES");
}
```

---

## On-Chain Integration

### Contract Reads (viem)

```typescript
import { createPublicClient, http } from "viem";
import { sepolia } from "viem/chains";

const client = createPublicClient({
    chain: sepolia,
    transport: http("https://ethereum-sepolia-rpc.publicnode.com"),
});

// Read market data from contract
async function getOnChainMarket(marketId: number) {
    return await client.readContract({
        address: PREDICTION_MARKET_ADDRESS,
        abi: predictionMarketAbi,
        functionName: "getMarket",
        args: [BigInt(marketId)],
    });
}

// Read user's token balances
async function getTokenBalance(tokenAddress: string, userAddress: string) {
    return await client.readContract({
        address: tokenAddress,
        abi: erc20Abi,
        functionName: "balanceOf",
        args: [userAddress],
    });
}
```

### Contract Writes (for order matching)

```typescript
import { createWalletClient, http } from "viem";
import { privateKeyToAccount } from "viem/accounts";

const account = privateKeyToAccount(BACKEND_PRIVATE_KEY);
const walletClient = createWalletClient({
    account,
    chain: sepolia,
    transport: http("https://ethereum-sepolia-rpc.publicnode.com"),
});

// Mint token pairs during order matching
async function mintTokens(marketId: number, to: string, amount: bigint) {
    // Backend wallet needs USDC approval first
    const hash = await walletClient.writeContract({
        address: PREDICTION_MARKET_ADDRESS,
        abi: predictionMarketAbi,
        functionName: "mintTokens",
        args: [BigInt(marketId), to, amount],
    });
    return hash;
}
```

---

## Project Structure

```
backend/
├── src/
│   ├── index.ts                  -- Hono app entry point
│   ├── routes/
│   │   ├── markets.ts            -- /api/markets/* endpoints
│   │   ├── orders.ts             -- /api/orders/* endpoints
│   │   ├── portfolio.ts          -- /api/portfolio/* endpoints
│   │   └── resolution.ts         -- /api/resolution/* endpoints
│   ├── services/
│   │   ├── dedup.ts              -- AI deduplication logic
│   │   ├── orderbook.ts          -- Order matching engine
│   │   ├── blockchain.ts         -- viem contract interactions
│   │   └── pricing.ts            -- Price calculation + snapshots
│   ├── db/
│   │   ├── client.ts             -- PostgreSQL connection
│   │   ├── schema.sql            -- Table definitions
│   │   └── queries.ts            -- Prepared queries
│   └── types/
│       └── index.ts              -- Shared TypeScript types
├── package.json
├── tsconfig.json
└── .env
```

---

## Environment Variables

```
# Database
DATABASE_URL=postgresql://user:pass@localhost:5432/prediction_market

# Blockchain
RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
PREDICTION_MARKET_ADDRESS=0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37
MOCK_USDC_ADDRESS=0x7817a9C826F5D3F237F4577cbF422EE84dDF212d
BACKEND_PRIVATE_KEY=0x...   # wallet that executes trades on-chain

# AI
GEMINI_API_KEY=your-key

# Server
PORT=3001
```

---

## Event Listener (Optional Enhancement)

Listen to on-chain events to keep the database in sync:

```typescript
// Watch for new markets created directly on-chain
client.watchContractEvent({
    address: PREDICTION_MARKET_ADDRESS,
    abi: predictionMarketAbi,
    eventName: "MarketCreated",
    onLogs(logs) {
        for (const log of logs) {
            syncMarketToDb(log.args);
        }
    },
});

// Watch for resolutions from CRE workflow
client.watchContractEvent({
    address: PREDICTION_MARKET_ADDRESS,
    abi: predictionMarketAbi,
    eventName: "MarketResolved",
    onLogs(logs) {
        for (const log of logs) {
            updateMarketResolution(log.args);
        }
    },
});
```

---

## Hackathon MVP Scope

For the demo, prioritize these endpoints:

```
Must have:
  POST /api/markets/create         -- with AI dedup
  GET  /api/markets                -- list markets
  GET  /api/markets/:id            -- market detail
  POST /api/orders                 -- place order (with matching)
  GET  /api/orders/:marketId       -- orderbook
  GET  /api/portfolio/:address     -- user positions

Nice to have:
  GET  /api/markets/check-duplicate -- live dedup as user types
  DELETE /api/orders/:id           -- cancel order
  GET  /api/portfolio/:address/history -- trade history
  GET  /api/resolution/:marketId   -- resolution details

Skip for MVP:
  Event listener (sync manually or on request)
  WebSocket price updates (use polling)
  Rate limiting / auth middleware
```