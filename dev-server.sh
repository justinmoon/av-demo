#!/usr/bin/env bash
set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Track PIDs
declare -a PIDS

# Cleanup function
cleanup() {
  echo ""
  echo -e "${YELLOW}Stopping services...${NC}"
  for pid in "${PIDS[@]}"; do
    kill "$pid" 2>/dev/null || true
  done
  wait 2>/dev/null || true
  echo -e "${GREEN}Services stopped${NC}"
  exit 0
}

# Set up trap to cleanup on exit
trap cleanup SIGINT SIGTERM EXIT

echo -e "${BLUE}Building WASM and UI...${NC}"
npm run build

echo -e "${BLUE}Starting MoQ relay...${NC}"
MOQ_RELAY_BIN="${MOQ_RELAY_BIN:-$HOME/code/moq/moq/rs/target/debug/moq-relay}"
if [ ! -f "$MOQ_RELAY_BIN" ]; then
  echo "Building moq-relay..."
  (cd "$HOME/code/moq/moq/rs" && cargo build --bin moq-relay)
fi
$MOQ_RELAY_BIN \
  --listen 127.0.0.1:4443 \
  --tls-generate localhost,127.0.0.1 \
  --auth-public marmot \
  --web-http-listen 127.0.0.1:4443 > /tmp/moq-relay.log 2>&1 &
MOQ_PID=$!
PIDS+=($MOQ_PID)
echo -e "${GREEN}MoQ relay started (PID: $MOQ_PID) at http://127.0.0.1:4443${NC}"

echo -e "${BLUE}Starting Nostr relay...${NC}"
NOSTR_BIN="${NOSTR_BIN:-nostr-rs-relay}"
if ! command -v $NOSTR_BIN &> /dev/null; then
  echo "Warning: nostr-rs-relay not found. Install with: cargo install nostr-rs-relay"
  echo "Skipping Nostr relay..."
else
  mkdir -p /tmp/nostr-relay
  cat > /tmp/nostr-relay/config.toml << 'EOF'
[info]
relay_url = "ws://127.0.0.1:8880"
name = "marmot-dev-relay"
description = "Development Nostr relay for Marmot chat"

[network]
port = 8880
address = "127.0.0.1"

[limits]
messages_per_sec = 100
EOF
  $NOSTR_BIN --config /tmp/nostr-relay/config.toml > /tmp/nostr-relay.log 2>&1 &
  NOSTR_PID=$!
  PIDS+=($NOSTR_PID)
  echo -e "${GREEN}Nostr relay started (PID: $NOSTR_PID) at ws://127.0.0.1:8880${NC}"
fi

echo -e "${BLUE}Starting chat UI server...${NC}"
node apps/chat-ui/server.js --port 8890 > /tmp/chat-ui.log 2>&1 &
UI_PID=$!
PIDS+=($UI_PID)
echo -e "${GREEN}Chat UI started (PID: $UI_PID) at http://localhost:8890${NC}"

echo ""
echo -e "${GREEN}✓ All services running!${NC}"
echo ""
echo "Open in 3 different browsers (or incognito windows):"
echo "  http://localhost:8890"
echo ""
echo -e "${BLUE}NIP-07 Extension Support:${NC}"
echo "  Install Alby (https://getalby.com) or nos2x extension"
echo "  Click 'Connect Extension' to use your Nostr identity"
echo "  Or click 'Generate temp key' for testing"
echo ""
echo "To test with 3 participants:"
echo "  1. Browser 1: Connect extension → Copy your pubkey → share with Browser 2"
echo "  2. Browser 2: Connect extension → Copy your pubkey → share with Browser 1"
echo "  3. Browser 1: Create new chat → Paste Browser 2's pubkey → Copy invite link"
echo "  4. Browser 2: Join invite (paste invite link)"
echo "  5. Browser 1: Add Participant → Paste Browser 3's pubkey"
echo "  6. Browser 3: Connect extension → Join invite (paste link)"
echo "  7. All 3 can now send messages!"
echo ""
echo -e "${YELLOW}Press Ctrl+C to stop all services${NC}"
echo ""
echo "Logs:"
echo "  MoQ relay: tail -f /tmp/moq-relay.log"
echo "  Nostr:     tail -f /tmp/nostr-relay.log"
echo "  Chat UI:   tail -f /tmp/chat-ui.log"
echo ""

# Wait for all background processes
wait
