# MoQ + Marmot A/V — Project Plan (Browser‑first)

References
- Spec: `MOQ_MARMOT_AV_SPEC.md`
- Auth: `NOSTR_AUTH.md`
- MoQ fast chat bridge (optional accelerator): `MOQ_CHAT_SERVER.md`

## 0) Goals & Scope
- Deliver end‑to‑end encrypted text, audio, and video for Marmot groups over MoQ relays.
- Keep Marmot privacy semantics: relays never learn message class or membership; no plaintext leaves clients.
- Minimize infra: prefer self‑issued capabilities (Nostr signatures) over a centralized JWT gateway.
- Browser‑first demo; optional native audio CLI later.

Out of scope
- Server‑side decryption/transcoding; centralized identity directories; full Blossom implementation.

## 1) Workstreams
- A1. Auth (self‑issued capabilities + write‑proof) [NOSTR_AUTH.md]
- A2. Control plane over MoQ (wrappers track, content‑blind paging, cursoring)
- A3. MDK in WASM (MLS group state, commits, welcomes, app messages)
- A4. Encrypted directory (media discovery)
- A5. Audio track encryption (Opus; nonce/generation schedule; AEAD hooks)
- A6. Video track encryption (VP8→H264/H265/AV1; codec plaintext exceptions)
- A7. Blobs fast‑path (publisher‑push; inline small; optional GET‑only proxy sidecar)
- A8. Optional: moq‑chat‑server accelerator (Nostr↔MoQ bridge for faster catch‑up)
- A9. Optional: native audio CLI (Rust) for interop/testing

## 2) Phased Delivery

### Phase 1 — Text over MoQ (Control Plane)
Tasks
- P1.1 Implement self‑issued capability verifier in `moq-relay` (accept `?cap=&sig=`) and dispatcher (JWT→cap→public).
- P1.2 Browser client: connect to relay with `cap` query param; subscribe to `marmot/<G>/wrappers`.
- P1.3 MDK wasm worker: process wrapper frames (commits/welcomes/app messages), advance epoch, decrypt chat.
- P1.4 Cursor/paging: fetch latest MoQ groups; if missing commits, pull older groups; store last sequence to resume.
- (Optional) P1.5 moq‑chat‑server: Nostr→MoQ republisher with content‑blind paging (speeds catch‑up, no protocol change).

Acceptance
- Cold start: render last N messages with ≤ 3 page requests; live tail < 100 ms.
- Reconnect from stored cursor; no Nostr fetches required.
- Privacy: single wrappers stream; relay does not learn message class.

### Phase 2 — Encrypted Audio (Opus)
Tasks
- P2.1 Encrypted directory: define/apply MLS application message schema listing track labels/configs; rotate per epoch.
- P2.2 Key derivation: per‑sender per‑track base via MLS exporter; generation keyed by MSB of 32‑bit nonce; cache prior gen/epoch ~10 s.
- P2.3 Frame AEAD: Opus fully encrypted; AAD binds (version, `<G>`, track `label`, epoch, seq/frame, keyframe flag false).
- P2.4 Publisher/subscriber: publish to `marmot/<G>/<label>`/subscribe & decrypt; handle epoch/generation rotation.

Acceptance
- 1:1 and small multi‑party audio calls; seamless epoch rotations; < 200 ms end‑to‑end latency typical WAN.
- No decrypt failures beyond transient reordering window; recoverable on reconnect.

### Phase 3 — Encrypted Video (+ Audio)
Tasks
- P3.1 VP8 first: plaintext header 1B/10B; bounded re‑encrypt on header collisions.
- P3.2 H.264/H.265: encrypt VCL; plaintext minimal non‑VCL; bounded re‑encrypt to avoid start‑codes.
- P3.3 AV1: OBU header/plain size handling and last‑OBU size quirk; bounded re‑encrypt.
- P3.4 Screenshare track; directory updates; label rotations.

Acceptance
- Multi‑party calls with video; smooth epoch/key rotations; acceptable CPU/bandwidth on modern laptops.

### Phase 4 — Blobs Fast‑Path (Text Attachments)
Tasks
- P4.1 Publisher‑push: publish ciphertext to `marmot/<G>/blob/<hash>` then reference in wrapper imeta.
- P4.2 Inline small blobs in wrappers (threshold configurable).
- (Optional) P4.3 GET‑only proxy sidecar: fetch Blossom by `<hash>`, stream to MoQ; optional on‑disk cache.

Acceptance
- Attachments load over existing QUIC session with no Blossom round‑trip in common case; ciphertext only on server.

### Phase 5 — Optional Native Audio CLI (Rust)
Tasks
- P5.1 Rust CLI using `moq-lite` + `hang` + MDK; mirrors Phase 2 behavior.
- P5.2 Interop tests with browser client.

Acceptance
- CLI joins existing group, plays/produces encrypted audio, epoch rotations OK.

## 3) Interfaces & Data
- Group root: `marmot/<G>`, with `<G> = hex(MLS-Exporter("moq-group-root-v1", group_id, 16))`.
- Wrappers track: raw wrapper JSON bytes; MoQ groups sized by N frames or T ms; client cursor = last (group,frame) seq.
- Directory: MLS application message `{ tracks: [{label, kind, codec, params, simulcast?}] }`.
- AEAD AAD: `version, <G>, label, epoch, group_seq, frame_idx, keyframe?`.
- Nonce: 32‑bit counter per track; MSB = generation; remainder from (group_seq, frame_idx). Uniqueness under reconnect required.

## 4) Dependencies
- Browser: WebTransport (fallback WS polyfill), WebCodecs/WebAudio; NIP‑07 for signing; wasm‑bindgen for MDK/OpenMLS worker.
- Relay: `moq-relay` patch for self‑issued caps; optional write‑proof for ingest.
- Optional: moq‑chat‑server to speed wrapper paging/catch‑up.

## 5) Observability & Tests
- Metrics: decrypt_failures, epoch_rotations, directory_updates, audio/video latency (end‑to‑end), page_fetch_ms, live_tail_ms.
- Unit: key derivation labels, nonce/generation mapping, directory parsing, AAD integrity.
- Integration: epoch churn, reconnect from stale cursor, packet loss/reorder; audio intelligibility; video playback.
- Load: 10k wrappers catch‑up; 3–8 participants A/V with epoch rotations.

## 6) Risks & Mitigations
- Browser support gaps → keep WS polyfill; test Chrome first.
- MLS churn under loss → commit replay buffer, short key cache, re‑welcome fast path.
- Nonce uniqueness → deterministic mapping + persistent counter per track; avoid reuse on reconnects.
- Codec quirks → bounded re‑encrypt loops; cap attempts; drop frame on repeated collision.
- Privacy regressions → single wrappers stream; random roots/labels; no protocol split.

## 7) Timeline (aggressive)
- Week 1–2: Phase 1 (Text), self‑caps in relay, MDK wasm worker, wrappers paging.
- Week 3–4: Phase 2 (Audio) — directory, AEAD, nonce/generation; 1:1 and small group calls.
- Week 5–6: Phase 3 (Video) — VP8 then H.264/AV1; screenshare.
- Week 7: Phase 4 (Blobs) — publisher‑push and inline; optional proxy.
- Week 8+: Phase 5 (Native audio CLI) & hardening.

## 8) Deliverables
- Browser demo app (text → audio → video) with docs and scripts.
- Relay patch: self‑issued capabilities; optional write‑proof.
- Directory/AEAD spec docs in `MOQ_MARMOT_AV_SPEC.md`.
- Optional: moq‑chat‑server binary (Nostr↔MoQ bridge) for faster catch‑up.
- Optional: native audio CLI.
