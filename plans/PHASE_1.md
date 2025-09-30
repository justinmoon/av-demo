# Control Plane MVP — Step Plan (Text over MoQ)

Goal
- Ship a browser-first text control plane over MoQ: one wrappers track per group, MDK handles MLS locally, relays remain content‑blind. Keep the path to audio/video open by reusing the same root and paging.

Short answer to “MDK in WASM?”
- Yes. For a browser MVP, MDK (OpenMLS + storage) must run in WASM (WebWorker) to process wrappers, advance epochs, and decrypt application messages. Use in‑memory storage; IndexedDB can follow later.

Track and Privacy
- Group root: `marmot/<G>` where `<G> = hex(MLS-Exporter("moq-group-root-v1", mls_group_id, 16))`.
- Single control‑plane track: `marmot/<G>/wrappers` carrying raw Nostr kind 444/445 bytes (content‑blind paging).
- Relays must not learn message class or membership from paths; all auth via URL query tokens.

Acceptance (end of Step 6)
- Two clients exchange and render text messages over MoQ using self‑issued capabilities (no centralized JWT), with cold‑start catch‑up and live tailing.

Step 1 — Get dependencies compiling in WASM
- Target: `wasm32-unknown-unknown`.
- Crates
  - Include `mdk-core` and `mdk-memory-storage` only for browser builds.
  - Exclude `mdk-sqlite-storage` in WASM.
  - Ensure `openmls`, `openmls_rust_crypto`, and `openmls_basic_credential` compile to WASM (RustCrypto backend).
- Create `mdk-wasm` wrapper crate
  - Crate type `cdylib`; use `wasm-bindgen`.
  - Minimal exports:
    - `init(user_pubkey_hex: string)`
    - `ingest_wrapper(json_bytes: Uint8Array) -> { kind: "application"|"commit"|"proposal"|"welcome"|"external"|"unprocessable", message?: DecryptedEvent }`
    - `create_message(rumor_json: string) -> Uint8Array` (wrapper bytes), for harness/tests
  - Optional: cursor helpers stored inside the worker (for resume).
- Build/test
  - `rustup target add wasm32-unknown-unknown`
  - `cargo build -p mdk-wasm --target wasm32-unknown-unknown`
  - Optional smoke test via `wasm-pack test --chrome` or a small HTML page that loads the module.

Step 2 — Automated demo of two clients (no MoQ)
- Purpose: validate MLS churn + chat without transport complexity.
- Harness options
  - Playwright test launching two pages that communicate via `BroadcastChannel` (or SharedWorker) to exchange wrapper bytes in order.
  - Simpler alternative: one page with two `mdk-wasm` workers and an in‑memory message queue.
- Flow
  - Page A: create group, send N application messages (+ one self‑update commit).
  - Page B: receives wrappers, advances epochs via MDK, renders decrypted messages; replies.
- Acceptance
  - Deterministic success for N messages; at least one epoch rotation processed on both ends.

Step 3 — Rust test: MDK chat over local MoQ relay
- Relay
  - Run `moq/rs/moq-relay` with dev config; enable `auth.public` or provide a dev JWT key.
- Test (Rust)
  - Use `moq-native` to publish `marmot/<G>/wrappers` and subscribe in a second client.
  - Sender: `MDK::create_message` → wrapper JSON bytes → write frames to `wrappers` track (content‑blind page by N/T policy).
  - Receiver: read frames → `MDK::process_message` → assert decrypted messages match.
- Acceptance
  - End‑to‑end send/receive of M messages with 0 decrypt failures; handles one commit.

Step 4 — WASM demo over MoQ (JWT first)
- Browser integration
  - Use `@kixelated/moq` to connect to local relay with `?jwt=<token>` (generate via `rs/moq-token-cli` or dev token file).
  - Subscribe to `marmot/<G>/wrappers`; send frames to the `mdk-wasm` worker; render chat timeline.
- Publisher path
  - Reuse the Rust publisher from Step 3 or a minimal `moq‑chat‑server` to populate `wrappers`.
- Acceptance
  - Chat renders over real MoQ; reload resumes tailing; acceptable latency.

Step 5 — Implement Nostr auth functionality on MoQ
- moq‑relay changes
  - `rs/moq-relay/src/connection.rs`: parse `cap` and `sig` from the URL query (alongside `jwt`).
  - `rs/moq-relay/src/auth.rs`: add `verify_cap(path, cap_bytes, sig)` per plans/NOSTR_AUTH.md:
    - Canonicalize JSON (JCS), compute `sha256`, verify BIP‑340 Schnorr over secp256k1.
    - Enforce `exp/nbf`, host in `aud`, and path containment by `root`.
    - Build `AuthToken { root, subscribe[], publish[] }` and integrate dispatcher order: JWT → self‑cap → public.
  - Tests: expiry, nbf, aud mismatch, path reduction, read‑only/write‑only behavior.

Step 6 — WASM demo using Nostr auth
- Capability issuance (browser)
  - Build JCS payload `{ver,kid,root,get:["wrappers"],put:[],exp,aud}`.
  - Use NIP‑07 to produce BIP‑340 Schnorr signature over `sha256(payload)`.
  - Connect via `@kixelated/moq` with `?cap=<b64url>&sig=<hex>` and render chat as in Step 4.
- Acceptance
  - Remove JWT; demo works end‑to‑end with self‑issued capabilities.

Notes & Risks
- WASM: exclude sqlite; use `mdk-memory-storage`. Keep heavy crypto on worker threads.
- Paging/cursor: start simple; add group sequence u64 in first frame per MoQ group if using `moq‑chat‑server` for fast resume.
- Privacy: keep group roots/labels random‑looking; avoid npubs in URL paths; rely on capabilities for access control.

