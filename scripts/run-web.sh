#!/usr/bin/env bash
set -euo pipefail

relay_port="54943"
server_port="8890"
nostr_port="7447"
hosts="localhost,127.0.0.1"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --relay-port)
      relay_port="$2"
      shift 2
      ;;
    --server-port)
      server_port="$2"
      shift 2
      ;;
    --nostr-port)
      nostr_port="$2"
      shift 2
      ;;
    --hosts)
      hosts="$2"
      shift 2
      ;;
    -h|--help)
      cat <<USAGE
Usage: ${0##*/} [--relay-port PORT] [--server-port PORT] [--nostr-port PORT] [--hosts HOSTS]

Bootstraps the local Nostr + MoQ relays and Marmot chat UI server for manual testing.
  --relay-port   TCP port for moq-relay listen/web endpoints (default: $relay_port)
  --server-port  HTTP port for chat UI server (default: $server_port)
  --nostr-port   WebSocket port for nostr-rs-relay (default: $nostr_port)
  --hosts        Hostnames for --tls-generate (default: $hosts)

The script will build the wasm bundle + UI, ensure the relay binary exists,
run all processes, and block until interrupted (Ctrl+C).
USAGE
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      exit 1
      ;;
  esac
done

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MOQ_ROOT="${MOQ_ROOT:-$HOME/code/moq/moq}"
RELAY_ROOT="$MOQ_ROOT/rs"
RELAY_BIN="$RELAY_ROOT/target/debug/moq-relay"
source "$ROOT_DIR/scripts/lib-nostr.sh"

if [[ ! -d "$MOQ_ROOT" ]]; then
  echo "error: expected moq repo at $MOQ_ROOT (override with MOQ_ROOT env var)" >&2
  exit 1
fi

if [[ ! -x "$RELAY_BIN" ]]; then
  echo "Building moq-relay (debug) at $RELAY_BIN" >&2
  (cd "$RELAY_ROOT" && cargo build -p moq-relay)
fi

cd "$ROOT_DIR"

if [[ ! -d node_modules ]]; then
  echo "Installing npm dependencies" >&2
  npm install
fi

echo "Building wasm bundle and chat UI" >&2
npm run build

cleanup() {
  local exit_code=$?
  stop_nostr_relay
  if [[ -n "${CHAT_PID:-}" ]]; then
    kill "$CHAT_PID" 2>/dev/null || true
  fi
  if [[ -n "${RELAY_PID:-}" ]]; then
    kill "$RELAY_PID" 2>/dev/null || true
  fi
  wait 2>/dev/null || true
  exit "$exit_code"
}

trap cleanup INT TERM EXIT

start_nostr_relay "$nostr_port" "$hosts"

echo "Starting moq-relay on 127.0.0.1:$relay_port" >&2
"$RELAY_BIN" \
  --listen "127.0.0.1:$relay_port" \
  --tls-generate "$hosts" \
  --auth-public marmot \
  --web-http-listen "127.0.0.1:$relay_port" \
  > >(sed 's/^/[relay] /') 2> >(sed 's/^/[relay] /' >&2) &
RELAY_PID=$!

sleep 0.5

echo "Starting chat UI server on http://127.0.0.1:$server_port" >&2
node apps/chat-ui/server.js --port "$server_port" \
  > >(sed 's/^/[chat-ui] /') 2> >(sed 's/^/[chat-ui] /' >&2) &
CHAT_PID=$!

cat <<INFO

Marmot chat UI is ready.
  Relay:   https://127.0.0.1:$relay_port/marmot
  UI:      http://127.0.0.1:$server_port/
  Nostr:   ws://127.0.0.1:$nostr_port/

Open two browser tabs:
  http://127.0.0.1:$server_port/?role=creator&relay=http://127.0.0.1:$relay_port/marmot&nostr=ws://127.0.0.1:$nostr_port/&session=demo
  http://127.0.0.1:$server_port/?role=joiner&relay=http://127.0.0.1:$relay_port/marmot&nostr=ws://127.0.0.1:$nostr_port/&session=demo

Press Ctrl+C to stop both servers.
INFO

wait
