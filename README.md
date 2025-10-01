# MoQ + Marmot A/V Demo (Workspace Index)

- plans/MOQ_MARMOT_AV_PLAN.md — Project plan (browser‑first: Text → Audio → Video)
- MOQ_MARMOT_AV_SPEC.md — Spec for MoQ + Marmot A/V (auth, directory, AEAD, tracks)
- MOQ_CHAT_SERVER.md — Binary Nostr relay (MoQ chat accelerator) design
- NOSTR_AUTH.md — Self‑issued caps + write‑proof auth (JWT alternative)

Notes
- Keep docs as the source of truth. Code should reference these docs instead of duplicating details.
- For implementation, create a new app in a separate folder (e.g., apps/marmot-moq-demo) and link back here.
- A reproducible toolchain is available via `nix develop .#`. The default shell exposes `cargo`, `wasm-bindgen`, and wasm-aware `clang` wrappers so `cargo build -p marmot-chat --target wasm32-unknown-unknown` succeeds without extra host setup.
- The `marmot-chat` crate now exposes the shared scenario fixtures plus identity-centric wasm bindings (`create_identity`, `public_key`, `create_message`, `ingest_wrapper`, `accept_welcome`, `merge_pending_commit`) via `serde_wasm_bindgen` so browser code can pass plain JS objects.
- Browser demo: `npm run build:wasm && npm test` runs the Playwright harness in `tests/`, exercising the Step‑2 flow (two identities, welcome, commit rotation, post-rotation messaging).
- Wasm unit test: `nix develop .# -c wasm-pack test --node crates/marmot-chat` runs the wasm-bindgen test module that mirrors the JS harness without Playwright.
- Browser chat UI: `npm run build` builds the wasm bundle and TypeScript client in `apps/chat-ui/`. Run a Nostr relay (e.g. `nostr-rs-relay --config ./config.toml` pointing at a temp dir) plus a MoQ relay (e.g. `moq-relay --listen 127.0.0.1:54840 --tls-generate localhost,127.0.0.1 --auth-public marmot --web-http-listen 127.0.0.1:54840`), then start `node apps/chat-ui/server.js --port 8889`. Open two tabs with `?role=alice&relay=http://127.0.0.1:54840/marmot&nostr=ws://127.0.0.1:7447/&session=test` and `?role=bob&relay=...` to chat via MoQ.
- justfile shortcuts:
  - `just wasm-test` — run the wasm regression suite (`wasm-pack test --node crates/marmot-chat`).
  - `just web-test` — build everything and execute the Playwright Step‑4 run (wraps `scripts/test-web.sh`).
  - `just dev` — build bundles, launch nostr-rs-relay + moq-relay, and start the chat UI server; CTRL+C stops all three. Override ports with `just dev relay_port=… nostr_port=…`.
  - `just playwright` — run all Playwright specs (`npm run test`).
  - `just chat-dev` — rebuild the UI and launch only the chat server (defaults to port 8890).
