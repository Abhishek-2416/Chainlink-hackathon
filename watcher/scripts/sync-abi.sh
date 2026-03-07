#!/usr/bin/env bash
# Sync PredictionMarket ABI from contracts folder.
# Run from watcher/ or repo root. Requires: forge build in contracts/

set -e
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WATCHER_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(cd "$WATCHER_DIR/.." && pwd)"
CONTRACTS_ARTIFACT="$REPO_ROOT/contracts/out/PredictionMarket.sol/PredictionMarket.json"
WATCHER_ABI="$WATCHER_DIR/abi/PredictionMarket.json"

if [[ ! -f "$CONTRACTS_ARTIFACT" ]]; then
  echo "Contract artifact not found. Run: cd contracts && forge build"
  exit 1
fi

cp "$CONTRACTS_ARTIFACT" "$WATCHER_ABI"
echo "Synced ABI to $WATCHER_ABI"
