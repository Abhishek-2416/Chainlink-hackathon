# Watcher Service Spec

## Overview

The watcher is a **standalone Rust service** that listens to on-chain events from the PredictionMarket contract and keeps the PostgreSQL database in sync. It runs independently from the backend API (Hono/Bun). See `backend/backend spec.md` for the full platform goal.

**Scope:** The watcher folder contains only the Rust event-watcher service. No API routes beyond health/status.

## Responsibilities

| Event | Action |
|-------|--------|
| `MarketCreated` | Insert or update market in DB (update tx_hash, yes_token_address, no_token_address) |
| `OrderFilled` | Insert trade, update matching order status + tx_hash + filled_at |
| `MarketResolved` | Update market status to 'resolved', set outcome, resolved_at |
| `MarketCancelled` | Update market status to 'cancelled' |
| `WinningsRedeemed` | Record redemption for analytics (optional) |

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Backend   │     │   Watcher   │     │  PostgreSQL │
│   (Hono)    │     │   (Rust)    │     │             │
└──────┬──────┘     └──────┬──────┘     └──────┬──────┘
       │                   │                   │
       │  fillOrder()       │  getLogs()        │
       │ ──────────────────►│                   │
       │                   │  INSERT/UPDATE    │
       │                   │ ─────────────────►│
       │                   │                   │
       │  read from DB     │                   │
       │ ◄─────────────────────────────────────│
```

## Tech Stack

- **Runtime:** Rust (Tokio async runtime)
- **HTTP:** Axum (health/status endpoint)
- **Blockchain:** ethers-rs (event parsing via topic0, RPC getLogs)
- **Database:** sqlx (PostgreSQL)
- **Logging:** tracing + tracing-subscriber (structured logs)

## Contract Interfaces

Event definitions and ABIs are sourced from the **contracts** folder:

- **Source:** `contracts/src/PredictionMarket.sol`
- **ABI:** `contracts/out/PredictionMarket.sol/PredictionMarket.json` (after `forge build`)
- **Watcher copy:** `watcher/abi/PredictionMarket.json`

Sync ABI after contract changes:

```bash
./watcher/scripts/sync-abi.sh
# Or: cp contracts/out/PredictionMarket.sol/PredictionMarket.json watcher/abi/
```

## Event Sources

All events come from **PredictionMarket.sol**:

- `MarketCreated(uint256 indexed marketId, bytes32 questionHash, address creator, address yesToken, address noToken, uint256 resolutionTimestamp)`
- `OrderFilled(uint256 indexed marketId, address indexed buyer, Outcome outcome, uint256 shares, uint256 cost)`
- `MarketResolved(uint256 indexed marketId, Outcome outcome)`
- `MarketCancelled(uint256 indexed marketId)`
- `WinningsRedeemed(uint256 indexed marketId, address indexed user, uint256 amount)`

## Block Cursor

The watcher tracks progress in `watcher_cursor`:

```sql
watcher_cursor (chain_id, contract_address, last_block, updated_at)
```

- Polls `getLogs` from `last_block + 1` to `current_block` (in batches)
- Updates `last_block` after each successful batch
- On first run: bootstrap from `current_block - 1000` or config

## Database Schema (from backend spec)

Relevant tables the watcher writes to:

- **markets** — `tx_hash`, `yes_token_address`, `no_token_address`, `status`, `outcome`, `resolved_at`
- **trades** — insert on OrderFilled
- **orders** — update `status`, `tx_hash`, `filled_at` on OrderFilled

## Configuration

| Env Var | Description |
|---------|-------------|
| `DATABASE_URL` | PostgreSQL connection string |
| `RPC_URL` | Ethereum RPC (e.g. Sepolia) |
| `PREDICTION_MARKET_ADDRESS` | Contract address |
| `CHAIN_ID` | Chain ID (11155111 for Sepolia) |
| `BATCH_SIZE` | Blocks per poll (default 2000) |
| `POLL_INTERVAL_MS` | Delay between polls (default 1000) |
| `PORT` | HTTP port for health (default 3002) |

## Endpoints

| Method | Path | Description |
|--------|------|-------------|
| GET | `/health` | Liveness probe |
| GET | `/status` | Last processed block, chain_id |

## Idempotency

For restarts, the watcher uses `watcher_cursor.last_block`. To avoid duplicate trades on re-processing, add a unique constraint:

```sql
CREATE UNIQUE INDEX IF NOT EXISTS idx_trades_dedup
ON trades (tx_hash, market_id, buyer_address, shares, cost);
```

Then use `ON CONFLICT` in the trades INSERT. (Current implementation does plain INSERT.)

## Logging

Uses `tracing` with structured fields. Set log level via `RUST_LOG`:

```bash
RUST_LOG=watcher=info,info cargo run   # default
RUST_LOG=watcher=debug cargo run       # verbose
```

## Running

```bash
cd watcher
cargo run
```

Or with env:

```bash
DATABASE_URL=postgresql://user:pass@localhost:5432/prediction_market \
RPC_URL=https://ethereum-sepolia-rpc.publicnode.com \
PREDICTION_MARKET_ADDRESS=0x45e7911Af8c31bDeDf8A586BeEd8efEcACEb9c37 \
CHAIN_ID=11155111 \
cargo run
```
