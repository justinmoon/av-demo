#!/usr/bin/env bash
# Shared helpers for launching a local nostr-rs-relay instance.

start_nostr_relay() {
  local port="$1"
  local hosts="$2"
  local bin="${NOSTR_RELAY_BIN:-nostr-rs-relay}"
  if ! command -v "$bin" >/dev/null 2>&1; then
    echo "error: nostr relay binary '$bin' not found (set NOSTR_RELAY_BIN)" >&2
    return 1
  fi

  local tmpdir
  tmpdir="$(mktemp -d "${TMPDIR:-/tmp}/marmot-nostr.XXXXXX")"
  local config="$tmpdir/config.toml"
  mkdir -p "$tmpdir/db"

  cat >"$config" <<CONFIG
[info]
relay_url = "ws://127.0.0.1:${port}"
name = "Marmot Test Relay"
description = "Ephemeral relay for Marmot MoQ demo"

[database]
data_directory = "${tmpdir}/db"

[network]
port = ${port}
address = "127.0.0.1"

[limits]
messages_per_sec = 1000
max_event_bytes = 262144
max_ws_message_bytes = 262144
max_ws_frame_bytes = 262144
subscription_count_per_client = 128

[verified_users]
mode = "disabled"
CONFIG

  "$bin" --config "$config" \
    >"$tmpdir/relay.log" 2>&1 &
  local pid=$!

  NOSTR_RELAY_PID="$pid"
  NOSTR_RELAY_DATA_DIR="$tmpdir"
  NOSTR_RELAY_CONFIG="$config"
  NOSTR_RELAY_PORT="$port"

  for _ in $(seq 1 40); do
    if nc -z 127.0.0.1 "$port" >/dev/null 2>&1; then
      echo "[nostr-relay] started on ws://127.0.0.1:${port}" >&2
      return 0
    fi
    sleep 0.1
  done

  echo "error: nostr relay failed to start; log: $tmpdir/relay.log" >&2
  stop_nostr_relay
  return 1
}

stop_nostr_relay() {
  if [[ -n "${NOSTR_RELAY_PID:-}" ]]; then
    kill "$NOSTR_RELAY_PID" >/dev/null 2>&1 || true
    wait "$NOSTR_RELAY_PID" 2>/dev/null || true
    unset NOSTR_RELAY_PID
  fi
  if [[ -n "${NOSTR_RELAY_DATA_DIR:-}" && -d "$NOSTR_RELAY_DATA_DIR" ]]; then
    rm -rf "$NOSTR_RELAY_DATA_DIR"
    unset NOSTR_RELAY_DATA_DIR
  fi
}
