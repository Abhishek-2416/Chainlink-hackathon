# Prediction Market Backend (Rust + Axum)

Backend for the prediction market platform. Handles order creation, orderbook logic, user portfolios, and redemption status. Integrates with the `PredictionMarket.sol` contract for on-chain order execution.

## Features

- **Order Creation Flow**: Quote → Sign (EIP-712) → Submit → Fill on-chain
- **Orderbook Logic**: Aggregates pending orders by price; derives implied probability (yes_price, no_price) for share costing
- **User Orders & Value**: Get orders by user, total value
- **Action Mapper**: Tracks required blockchain actions (sign_eip712, fillOrder, redeemWinning)
- **Redemption**: User redeems on-chain; backend provides status and instructions

## Tech Stack

- **Runtime**: Rust
- **Framework**: Axum
- **Database**: PostgreSQL + SQLx
- **Blockchain**: ethers-rs (EIP-712, contract calls)

## Database Schema

See `migrations/001_initial_schema.sql`:

- `markets` - Market metadata (mirrors on-chain)
- `orders` - Signed orders (pending/filled)
- `trades` - From OrderFilled events (watcher populates)
- `price_snapshots` - Price history
- `orderbook_levels` - Cached orderbook (optional)
- `action_mapper` - User actions → required tx
- `watcher_cursor` - Block sync state

## API Endpoints

### Orders
- `POST /api/orders/quote` - Get order payload for signing
- `POST /api/orders` - Submit signed order
- `GET /api/orders/:marketId` - Orderbook
- `GET /api/orders/user/:address` - User's orders + total value

### Markets
- `GET /api/markets` - List markets
- `GET /api/markets/:marketId` - Market detail + orderbook

### Portfolio
- `GET /api/portfolio/:address` - Positions, open orders
- `GET /api/portfolio/:address/history` - Trade history
- `GET /api/portfolio/:address/redemption-status` - Redeemable markets

### Actions
- `GET /api/actions/user/:address` - Pending actions for user
- `GET /api/actions` - List all pending actions

## Orderbook → Probability

The orderbook affects share pricing:

- **YES bids** at price 72¢ → implied 72% probability YES wins
- **NO bids** at price 28¢ → implied 28% probability YES wins (72% NO)
- `shares = floor(cost / (price/100))` — e.g. $5 at 72¢ → 6,944,444 shares

Price discovery: last trade > volume-weighted orderbook mid > 50/50 default.

## Setup

```bash
cp .env.example .env
# Edit .env with DATABASE_URL, RPC_URL, BACKEND_PRIVATE_KEY

# Create DB and run migrations
createdb prediction_market
sqlx migrate run

# Build (requires DATABASE_URL for sqlx compile-time check, or use SQLX_OFFLINE=true)
cargo build

# Run
cargo run
```

For offline builds without a database:
```bash
SQLX_OFFLINE=true cargo build
```

## Contract Flow

1. **Quote**: Backend fetches nonce, computes shares from cost+price, returns EIP-712 payload
2. **Sign**: User signs with wallet (MetaMask, etc.)
3. **Submit**: Backend verifies, stores order, calls `fillOrder(order, signature)` on-chain
4. **Redeem**: After resolution, user calls `redeemWinning(marketId, amount)` on contract
