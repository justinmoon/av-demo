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
- Rust/wasm regression: `just wasm-test` (or `nix develop .# -c wasm-pack test --node crates/marmot-chat`) exercises the deterministic Phase‑4 backlog in pure Rust, proving Bob can ingest Alice’s wrappers with no JS glue.
- Browser regression suite: `npm run test` runs the Playwright specs (`tests/step2-demo.spec.js` for the BroadcastChannel flow plus `tests/step4-chat.spec.js` for the MoQ transport). Use `just web-test` to focus on the MoQ run.
- Build pipeline: `npm run build` (or `just build`) compiles the `marmot-chat` wasm bundle and the SolidJS UI in `apps/chat-ui/dist/`.
- Dev loop: `just dev` launches an ephemeral `nostr-rs-relay`, `moq-relay`, and the chat UI server with fresh builds. Visit `http://127.0.0.1:8890/` (no query params) and follow the onboarding — choose NIP‑07 or a developer secret, create an invite, share it, then chat over MoQ.
- Targeted helpers:
  - `just chat-dev` — rebuild the UI and serve it without relays (useful when pointing at existing infra).
  - `just relay-dev` — run only `moq-relay` with self-signed TLS for manual experiments.
