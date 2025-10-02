#!/usr/bin/env bash
set -e

# Colors for output
GREEN='\033[0;32m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}Building WASM and UI...${NC}"
npm run build

echo -e "${BLUE}Starting MoQ relay...${NC}"
MOQ_RELAY_BIN="${MOQ_RELAY_BIN:-$HOME/code/moq/moq/rs/target/debug/moq-relay}"
if [ ! -f "$MOQ_RELAY_BIN" ]; then
  echo "Building moq-relay..."
  (cd "$HOME/code/moq/moq/rs" && cargo build --bin moq-relay)
fi
$MOQ_RELAY_BIN --dev --bind 127.0.0.1:4443 > /tmp/moq-relay.log 2>&1 &
MOQ_PID=$!
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
  echo -e "${GREEN}Nostr relay started (PID: $NOSTR_PID) at ws://127.0.0.1:8880${NC}"
fi

echo -e "${BLUE}Starting chat UI server...${NC}"
node apps/chat-ui/server.js --port 8890 > /tmp/chat-ui.log 2>&1 &
UI_PID=$!
echo -e "${GREEN}Chat UI started (PID: $UI_PID) at http://localhost:8890${NC}"

# Create cleanup script
cat > /tmp/stop-dev-server.sh << EOF
#!/usr/bin/env bash
echo "Stopping services..."
kill $MOQ_PID $NOSTR_PID $UI_PID 2>/dev/null || true
echo "Services stopped"
rm /tmp/stop-dev-server.sh
EOF
chmod +x /tmp/stop-dev-server.sh

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
echo "  1. Browser 1: Connect extension → Create new chat"
echo "  2. Enter Browser 2's pubkey → Copy invite link"
echo "  3. Browser 2: Connect extension → Join invite (paste link)"
echo "  4. Browser 1: Add Participant → paste Browser 3's pubkey"
echo "  5. Browser 3: Connect extension → Join invite (paste link)"
echo "  6. All 3 can now send messages!"
echo ""
echo "To stop all services, run:"
echo "  /tmp/stop-dev-server.sh"
echo ""
echo "Logs:"
echo "  MoQ relay: tail -f /tmp/moq-relay.log"
echo "  Nostr:     tail -f /tmp/nostr-relay.log"
echo "  Chat UI:   tail -f /tmp/chat-ui.log"
