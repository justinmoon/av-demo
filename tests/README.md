# Phase 1 Step 2 – Browser Harness (No MoQ)

This harness exercises the `marmot-chat` control-plane bindings entirely in the browser.
Two Playwright-controlled pages talk over a `BroadcastChannel`, but all MLS work is
performed by the wasm module exported from `crates/marmot-chat`.

## Flow

1. **Bob (page B)** builds a key package with `create_key_package` and broadcasts it.
2. **Alice (page A)** creates an identity, builds the group with `create_group`, and
   sends the welcome wrapper to Bob.
3. Bob accepts the welcome via `accept_welcome`, joins the group, and the pages start
   exchanging encrypted application messages using `create_message` / `ingest_wrapper`.
4. Alice performs a `self_update`, broadcasts the resulting commit wrapper, and both
   pages call `merge_pending_commit` so the epoch rotation completes to the new state.
5. Messaging continues after the rotation to prove both sides advanced correctly.

## Running the wasm / Playwright tests

```bash
npm install                    # once
npm run build:wasm             # regenerates tests/pkg from the latest Rust code
npm test                       # headless Playwright run
npm run test:headed            # watch the pages interact in a real browser
```

The `npm run build:wasm` script wraps `wasm-pack build --target web`
and materialises bindings into `tests/pkg/`. That directory is git-ignored and is safe to
remove; just rerun the build step whenever the Rust code changes.

## Files

- `page-a.html` – Alice identity, group creation, outbound messaging
- `page-b.html` – Bob identity, welcome processing, inbound messaging
- `server.js` – static HTTP server with COOP/COEP headers for wasm
- `step2-demo.spec.js` – Playwright orchestration for the Step‑2 acceptance test

## Acceptance

✅ Deterministic success for the configured message count

✅ Commit / epoch rotation processed on both sides

✅ Post-rotation messages decrypt successfully
