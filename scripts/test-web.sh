#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MOQ_ROOT="${MOQ_ROOT:-$HOME/code/moq/moq}"
RELAY_ROOT="$MOQ_ROOT/rs"
RELAY_BIN="$RELAY_ROOT/target/debug/moq-relay"
source "$ROOT_DIR/scripts/lib-nostr.sh"

NOSTR_PORT="${NOSTR_PORT:-7447}"

if [ ! -d "$MOQ_ROOT" ]; then
  echo "error: expected moq repo at $MOQ_ROOT (override with MOQ_ROOT env var)" >&2
  exit 1
fi

if [ ! -x "$RELAY_BIN" ]; then
  echo "Building moq-relay (debug) at $RELAY_BIN" >&2
  (cd "$RELAY_ROOT" && cargo build -p moq-relay)
fi

cd "$ROOT_DIR"

if [ ! -d node_modules ]; then
  echo "Installing npm dependencies" >&2
  npm install
fi

echo "Building wasm bundle and chat UI" >&2
npm run build

PLAYWRIGHT_ARGS=("$@")

cleanup() {
  local exit_code=$?
  stop_nostr_relay
  exit "$exit_code"
}

trap cleanup INT TERM EXIT

start_nostr_relay "$NOSTR_PORT" "localhost,127.0.0.1"
export MARMOT_NOSTR_PORT="$NOSTR_PORT"
export MARMOT_NOSTR_URL="ws://127.0.0.1:${NOSTR_PORT}/"

echo "Running Playwright web test (tests/step4-chat.spec.js)" >&2
npx playwright test "${PLAYWRIGHT_ARGS[@]}" tests/step4-chat.spec.js
