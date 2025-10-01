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
