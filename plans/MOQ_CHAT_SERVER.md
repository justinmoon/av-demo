# MoQ Chat Server — Binary Accelerator for Marmot/Whitenoise

Goal: a QUIC/MoQ service that delivers Marmot wrapper events and encrypted blobs as fast binary streams, without changing MLS semantics or leaking message classes. Optional write‑through to Nostr for interop. Stateless by default; can rehydrate from Nostr on restart.

## Non‑Goals
- Do not decrypt MLS application messages or media.
- Do not re‑sign Nostr wrappers on behalf of users.
- Do not change Marmot wire formats or privacy model.

## High‑Level Architecture
- moq-relay (existing) — transport fan‑out with JWT auth.
- moq-chat-server (new) — stateless bridge/aggregator:
  - Nostr→MoQ: subscribe to group wrappers on Nostr, republish binary frames on a single MoQ track with content‑blind paging.
  - (Optional) MoQ ingest→Nostr: accept client pre‑signed wrappers; forward to Nostr relays; republish to MoQ stream.
  - (Optional) Blob proxy/publisher: serve encrypted blobs over MoQ.
- Auth Gateway (reuse `nostr-moq/gateway`) — issues JWTs for MoQ paths after Nostr challenge.

## Paths & Tracks
- Group root path: marmot/<hex_nostr_group_id> (public identifier; access is controlled via JWT; no membership leakage if tokens are scoped).
- Tracks under group root:
  - events (server publishes; clients subscribe): content‑blind stream of all group events (445, 444) in send order.
  - ingest/<label> (clients publish; server subscribes): pre‑signed wrapper upload (see Auth). label is random (see below) to avoid identity leakage.
  - blob/<hash> (optional; server publishes; clients subscribe): encrypted blob ciphertext keyed by SHA256(ciphertext).

Notes
- Single writer per track is respected: server is sole publisher of events and blob/*. Clients never publish to events.
- Identity privacy: no npubs in paths; label is random.

### Track Map (summary)
- Group root (privacy‑preserving): `marmot/<G>`, where `<G> = hex(MLS‑Exporter("moq-group-root-v1", mls_group_id, 16))`.
- Server‑published track:
  - `marmot/<G>/events` — all Nostr messages (kinds 444/445) the server has seen for the group, paged content‑blind; single writer = chat server.
- Client‑published ingest tracks (one per participant):
  - `marmot/<G>/ingest/<L(pk)>` — client uploads pre‑signed wrappers; `L(pk) = sha256(pubkey)`; only that pubkey can publish (enforced by path‑auth write proof or a scoped self‑cap).
- Optional blobs:
  - `marmot/<G>/blob/<hash>` — encrypted blobs published by clients (ciphertext hash).

Events: these are all the Nostr messages (kinds 444/445) that the server has seen for the group, carried verbatim on the `events` track. Clients validate/decrypt using MDK/OpenMLS. The server does not decrypt or re‑sign content.

## Wire Formats (binary)
- Events track: each MoQ frame payload is the raw Nostr event JSON bytes (UTF‑8), unmodified. Grouping (paging) is content‑blind:
  - Default: split into MoQ groups every N frames (e.g., 256) or T milliseconds (e.g., 1000ms), whichever comes first.
  - Add a small sequence number (u64) as MoQ group metadata via first frame prefix (8 bytes LE) for fast cursoring.
- Ingest frames: CBOR map { wrapper_json_bytes: bstr }.
- Blob frames: raw ciphertext chunks (size negotiated). First frame: 16‑byte header {version(1), flags(1), total_len(8), reserved(6)} then data; subsequent frames: data only.

## AuthN/AuthZ
- Read (events, blob/*): JWT scope to root = marmot/<group> and get = ["events", "blob"].
- Ingest publish (ingest/<label>): two options:
  1) JWT publish scope: put = ["ingest/<label>"] issued by gateway after Nostr challenge.
  2) Pubkey‑hash proof (no JWT for publish): client sends headers X-MoQ-PubKey: <pk>, X-MoQ-Signature: sig("authorize:"||path||nonce); relay verifies sig and SHA256(pk) == <label>. (Requires a small moq-relay auth extension.)

Gateway flow
- Client proves npub control (NIP‑98 or raw Schnorr), gateway checks MDK membership (optional), issues JWT limiting read to events/blob/* and write to ingest/<label> if enabled.

## Nostr Bridging
- Inputs:
  - Group filter: kind 445 (Group Event) + kind 444 (Welcome), with h tag = nostr_group_id.
  - Relays: configured list (writable for write‑through; readable for hydration).
- Nostr→MoQ:
  - Maintain per‑group in‑memory ring buffer of recent wrapper ids (EventId) for dedupe.
  - On event receipt: append to current MoQ group on events. Rotate group per (N,T) policy.
- MoQ ingest→Nostr (optional):
  - Accept ingest/<label> frame with wrapper_json_bytes.
  - Validate basic envelope: kind ∈ {444,445}, h tag matches path group, author npub present.
  - (Optional) Verify author proof via JWT or pubkey‑hash method.
  - POST to configured Nostr relays; dedupe by event id.
  - Echo to events immediately (optimistic) and reconcile on relay ACK if needed.

- Mode A — Dual‑write (recommended): client publishes pre‑signed wrappers to MoQ ingest and to at least one Nostr relay in parallel. MoQ provides low‑latency fan‑out; Nostr provides durability. Idempotency by Nostr EventId.
- Mode B — Server write‑through (optional): client publishes to MoQ ingest only; server posts to Nostr in background. Client sets a short fallback timer (e.g., 2–5s) and posts to Nostr itself if not persisted in time.
- UI acks: “fast delivered” = MoQ echo on `events`; “persisted” = first Nostr relay ACK (optionally signaled to peers via a small MLS app message).


## Blob Handling (privacy‑preserving)
- Baseline: publisher‑push to MoQ by clients
  - Sender publishes encrypted blob first to blob/<hash>.
  - Then sends wrapper referencing <hash> inside MLS imeta.
- Optional proxy (no full Blossom): GET‑only sidecar that, on demand, fetches https://blossom/<hash> and streams to blob/<hash>. No auth or deletes; optional local cache dir keyed by <hash>.
- Optional inline small blobs: for payloads ≤ X KiB, place ciphertext inside wrappers frames (or a companion inline-blobs marker) to avoid extra subscription.

## Statelessness & Hydration
- No DB required for MVP. On restart:
  - Resubscribe to Nostr, fetch last M wrappers per group, rebuild K most recent MoQ groups, resume publishing.
  - M,K tuneables (e.g., M=10k, K=40 groups × 256 frames).
- Optional persistence (Phase 4): LMDB for wrapper frames and blob cache to improve cold‑start latency.

## Client Contract
- Subscribe to marmot/<group>/events:
  - Fetch latest groups; try to decrypt in order. If missing MLS commits, request older groups until MLS advances; then follow live appends.
  - Remember last sequence (cursor) and resume from cursor on reconnect.
- To publish (optional): send pre‑signed wrapper (original JSON) to ingest/<label> and dual‑write to Nostr when bridge write‑through is disabled.
- Blobs: after decrypting wrapper with imeta x=<hash>, subscribe to blob/<hash> and stream ciphertext.
 - Optional Nostr reconciliation: after MoQ catch‑up, clients MAY run negentropy against the group’s relays to fetch only missing EventIds (bandwidth‑efficient gap fill). If a relay lacks negentropy, fall back to bounded filters (kinds 444/445, `#h`=group_id, time window) or GET‑by‑ids for the missing set.

## Config (TOML)
  # moq-chat-server.toml
  
  [nostr]
  relays_read = ["wss://relay1", "wss://relay2"]
  relays_write = ["wss://relay1"]          # optional
  connect_timeout_ms = 10000
  backfill_limit = 10000                    # wrappers per group on restart
  
  [moq]
  relay_url = "https://relay.example.com"
  root_prefix = "marmot"                    # path prefix before <group>
  page_frames = 256                         # frames per MoQ group
  page_duration_ms = 1000                   # max duration per MoQ group
  
  [auth]
  # Use JWT gateway for reads and (optionally) writes
  jwt_issuer_base = "https://gateway.example.com"
  # Or enable pubkey-hash proof for ingest writes (requires relay auth extension)
  pubkey_hash_proof = false
  
  [blob]
  proxy_enabled = false
  proxy_cache_dir = "/var/cache/moq-blobs" # optional, if enabled
  inline_small_kib = 16

## Milestones
1) Skeleton crate rs/moq-chat-server:
   - CLI, config load, logging, graceful shutdown.
2) Nostr→MoQ read path (MVP):
   - Nostr subscription by group; events republish to events with paging.
   - Dedupe by EventId. Basic metrics.
3) Auth integration:
   - Read JWT validation (moq-relay already). Gateway token flow docs.
   - (Optional) pubkey‑hash proof POC in moq-relay auth.
4) MoQ ingest→Nostr (optional):
   - Accept ingest/<label> frames; post to Nostr relays; echo to events.
5) Blob publisher (client‑push baseline):
   - Document client contract; relay pass‑through verified.
   - (Optional) GET‑only blob proxy sidecar.
6) Persistence (optional):
   - LMDB for wrapper frames + blob cache; crash‑safe rebuild.
7) Browser client glue:
   - MDK WASM (key packages, MLS processing), subscribe to wrappers, decrypt/render, blob fetch.
8) Perf & hardening:
   - Backpressure, rate limits, per‑group quotas, observability (Prometheus), chaos tests.

## Testing
- Unit: frame paging/rotation, dedupe, ingest validation.
- Integration: local Nostr relays + moq-relay; publish wrapper streams and verify client catch‑up timings.
- Property: idempotency (duplicates ignored), ordering preserved, no content‑aware decisions in paging.
- Load: 10k wrappers replay, measure cold start and live tail.

## Observability
- Counters: events_in, events_out, duplicates_dropped, ingest_in, ingest_post_ok/err, blob_requests, blob_bytes_out.
- Gauges: active_groups, active_connections, memory_bytes.
- Histograms: end‑to‑end publish→deliver latency, page_build_time_ms, backfill_time_ms.

## Security & Privacy
- No content parsing; paging is content‑blind.
- No plaintext at server; blobs stay ciphertext.
- JWT scopes restrict listing/fetching; disable /announced for public prefixes via auth.
- Do not log event content; hash EventId for correlation if needed.

Signature verification
- The server may skip per‑event Nostr signature verification on the MoQ hot‑path; clients SHOULD verify event signatures locally. When receiving large batches (e.g., page fetches), clients can use batch Schnorr verification to amortize checks and keep UI responsive (render on MLS decrypt, mark verified when batch completes).

## Hot Path Optimizations (notes)
- Per‑connection auth only: verify a self‑issued capability (or write‑proof) once at connect; skip per‑event server‑side signature checks. Clients verify MLS immediately and batch‑verify Nostr signatures asynchronously for large batches.
- No JSON in hot path: carry raw wrapper bytes verbatim on the `events` track; avoid parsing. Optionally add a group‑level binary envelope (e.g., CBOR + deflate) for page frames to reduce bytes; keep canonical JSON only when posting to Nostr.
- Zero‑copy buffering: use per‑group lock‑free ring buffers and pooled byte slices; write directly into MoQ group frames; avoid per‑frame allocations/logging.
- Fixed‑size paging: rotate MoQ groups by N frames (e.g., 256) or short time windows to balance latency and overhead.
- Inline small blobs: embed tiny ciphertexts in `events` pages; publish larger blobs to `blob/<hash>` ahead of time to eliminate extra RTTs.
- Async write‑through: echo to `events` immediately; post to Nostr in background with HTTP connection pooling and small micro‑batches; client fallback timer covers relay outages.
- Client pipeline: decrypt/render on MLS in a worker; batch Schnorr verify in a separate worker; update UI when batches complete.
- QUIC tuning: prefer low‑latency congestion control, partial reliability for media, and a single persistent session; minimize metrics on the hot path.

## Open Questions
- Should we compress wrapper JSON in CBOR for the MoQ fast path while preserving canonical wrapper on write‑through? (Trade‑off: extra CPU vs bytes.)
- How many groups/pages should the server retain in memory for fast scrollback before requiring Nostr backfill or LMDB?
- Do we need per‑sender rate limiting on ingest/<label>? (DOS window.)
- Fallback path if all Nostr relays are down and write‑through is enabled.

## Deliverables
- New crate: moq/rs/moq-chat-server (binary)
- Docs: quickstart, config, client contract, gateway integration.
- Optional patch: moq-relay auth extension for pubkey‑hash proof (feature‑gated).

## Timeline (aggressive)
- Week 1: Milestones 1–2.
- Week 2: Milestone 3; E2E demo (read‑only) with browser client.
- Week 3: Milestone 4; write‑through demo; bench catch‑up vs Nostr.
- Week 4: Milestone 5; optional blob proxy; basic dashboards.
- Week 5+: Persistence + hardening.
