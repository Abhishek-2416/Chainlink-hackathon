# Watcher Service

Rust service that listens to PredictionMarket contract events and keeps the PostgreSQL database in sync.

## Events Handled

| Event | DB Action |
|-------|-----------|
| `MarketCreated` | Update market tx_hash, yes_token_address, no_token_address |
| `OrderFilled` | Insert trade, update order status |
| `MarketResolved` | Update market status, outcome, resolved_at |
| `MarketCancelled` | Update market status to cancelled |
| `WinningsRedeemed` | Log (analytics) |

## Prerequisites

- Rust 1.70+
- PostgreSQL (shared with backend)
- `watcher_cursor` table (from backend schema)

## Build & Run

```bash
cargo build --release
cargo run
```

## Environment

Create `.env` or set:

```env
DATABASE_URL=postgresql://user:pass@localhost:5432/prediction_market
RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
PREDICTION_MARKET_ADDRESS=0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37
CHAIN_ID=11155111
BATCH_SIZE=2000
POLL_INTERVAL_MS=1000
PORT=3002
```

## Endpoints

- `GET /health` — Liveness
- `GET /status` — Last block, chain_id

## Contract ABI

ABI is synced from `contracts/out/PredictionMarket.sol/PredictionMarket.json`. After `forge build` in contracts:

```bash
./scripts/sync-abi.sh
```

See `SPEC.md` for full specification.
