# marmot-chat

Shared Rust core for the Marmot control-plane demo. The crate exposes:

- Deterministic Phase‑4 fixtures (`scenario` module) for reuse across tests and demos.
- WASM bindings (via `wasm-bindgen`) that wrap MDK/OpenMLS for browser clients.
- Wasm-bindgen tests that run entirely under `wasm-pack` to exercise the Bob bootstrap flow.

## Build

```bash
rustup target add wasm32-unknown-unknown
cargo build -p marmot-chat --target wasm32-unknown-unknown
```

## Tests

### Fast wasm regression
```bash
wasm-pack test --node crates/marmot-chat
```
This runs the wasm-bindgen suite (including the Bob bootstrap end-to-end scenario) entirely in Rust/WASM without any browser wiring.

### Web bundle (for browser apps)
```bash
wasm-pack build crates/marmot-chat --target web --out-dir ../../tests/pkg
```
Produces `pkg/marmot_chat.js`/`.wasm` for TypeScript apps. The command is wired into `npm run build:wasm` in the workspace `package.json`.

## Modules

- `scenario` – deterministic setup helpers (Alice/Bob identities, key package bundle export, backlog wrappers, live-frame generator, etc.).
- `wasm` (only compiled on `wasm32`) – `create_identity`, `accept_welcome`, `ingest_wrapper`, `merge_pending_commit`, and related bindings for JS.

## Feature flags

- `panic-hook` (default) – installs `console_error_panic_hook` when targeting wasm.

## Notes

- The crate relies on local MDK checkout paths under `/Users/justin/code/moq/mdk`. Ensure that repository is present when building.
- All wasm-facing types use `serde-wasm-bindgen`, so the browser can pass/receive plain JS structures.
