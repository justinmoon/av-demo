# MoQ + Marmot MLS: Text, Audio, Video (Browser)

## Scope
- Text chat, encrypted audio, and encrypted video over MoQ relays.
- Identity and group membership via Marmot/MLS; transport is MoQ.
- Privacy parity with Marmot over Nostr: relays do not learn message class, membership, or sender identity from paths.

## Identities & Groups
- User identity: Nostr pubkey (npub). Marmot/MDK binds MLS BasicCredential.identity to the npub bytes.
- MLS groups: created per Marmot MIPs; control messages (proposals, commits, welcomes) and application messages are serialized as “wrapper events” (same bytes as Nostr kind 444/445).
- Group root path (MoQ): random-looking label derived from exporter secrets to avoid linkability: `root = marmot/<G>` where `<G> = hex(MLS-Exporter("moq-group-root-v1", mls_group_id, 16))`.

## Auth (no central JWT required)
- Reads (subscribe): Self‑issued capabilities per NOSTR_AUTH.md — `cap` (JSON+JCS) + Schnorr `sig` using npub. Relay verifies signature and scopes; builds an `AuthToken` equivalent to JWT.
- Writes (publish) for ingest (if used): Pubkey‑hash proof per NOSTR_AUTH.md for `/ingest/<label>` (label = sha256(pubkey)). Relay verifies `(host,path,ts,nonce)` signature with pk.
- Alternatively keep JWT gateway; both modes are compatible.

## Control Plane (wrappers)
- Single MoQ track per group: `marmot/<G>/wrappers`.
- Frames: raw wrapper bytes (UTF‑8 JSON) exactly as Nostr would carry, in strict send order. Content‑blind paging into MoQ groups (e.g., every 256 frames or 1000 ms).
- Clients:
  - Subscribe; decrypt/process via MDK/OpenMLS to advance epoch and apply application messages.
  - Catch‑up: fetch latest MoQ groups; if decryption fails (missing commits), pull older groups until epoch advances; or ask for a standard MLS re‑welcome to jump to current epoch.
- Privacy: relays cannot distinguish commits from application; there is no separate “protocol” track.

## Encrypted Directory (media discovery)
- Directory message: small MLS application message listing current media tracks per sender:
  - For each track: `label`, `kind` (audio/video/screen), codec/config (RTPish or hang-ish minimal fields), optional simulcast, publish hints.
- `label` is a random-looking name derived from exporter secrets, e.g.: `label = hex(MLS-Exporter("moq-track-lbl-v1", sender_leaf || kind || epoch, 16))`.
- Rotation: re‑emit directory on epoch change or when tracks are added/removed. Subscribers update subscriptions.
- Privacy: track names convey nothing to relays; membership learned only by decrypting directory/application.

## Media Key Derivation
- Per sender S and track T, per epoch E:
  - `base = MLS-Exporter("moq-media-base-v1", sender_leaf || track_label || epoch_bytes, 32)`.
  - AEAD key/nonce per generation via ratchet: `K_gen, N_salt = HKDF(base, "k"/"n" || gen)`.
  - Generation: most‑significant byte of a 32‑bit frame counter nonce; rotate gen when MSB changes; cache previous gen/epoch keys for ~10 s.
- Security: FS/PCS via MLS exporter; no per‑sender auth via keys (consistent with DAVE); transport integrity via AEAD and AAD binding.

## Frame Encryption (SFrame‑style over MoQ/hang)
- Apply AEAD at the encoded frame boundary. Hang writes per‑frame payloads; we insert:
  - Nonce construction: from (group_sequence, frame_index) → 32‑bit counter + MSB(gen).
  - AAD binds: version label, group root `<G>`, track `label`, epoch number, (group_sequence, frame_index), and codec hints needed for replay protection (e.g., keyframe flag).
  - Ciphertext replaces the encoded payload; minimal plaintext header remains for decoder compatibility.
- Codec plaintext ranges (examples):
  - Opus: fully encrypted.
  - VP8: leave 1 byte (non‑keyframe) or 10 bytes (keyframe) per RFC7741.
  - VP9: fully encrypted (RTP payload descriptor already separate in RTP; in MoQ, we keep hang headers plaintext only).
  - H.264/H.265: encrypt VCL NALU payloads; leave minimal non‑VCL headers plaintext; after encrypt, rescan ciphertext to avoid start‑code sequences; if found, bump nonce and re‑encrypt up to a small bound.
  - AV1: leave 1B OBU header (+ optional ext) and optional size field as needed; handle last‑OBU size quirk as in DAVE.

## Publishing & Subscribing Media
- Publisher:
  - Generate track `label` and announce via encrypted directory.
  - For each encoded frame: compute nonce/gen, AEAD‑encrypt payload, write hang frame (timestamp header + ciphertext payload) to `marmot/<G>/<label>`.
- Subscriber:
  - Read directory; subscribe to each `label`.
  - For each frame: reconstruct nonce/gen from group+frame seq; try decrypt with current keys; fallback to cached gen/epoch; hand plaintext payload to decoder.

## Blobs (attachments in text)
- Baseline: sender publishes encrypted blob on `marmot/<G>/blob/<sha256(ciphertext)>` prior to sending wrapper; wrapper imeta carries `<hash>`. Receivers, after decrypt, subscribe to blob and stream ciphertext.
- Optional: GET‑only proxy sidecar that fetches Blossom by `<hash>` and republishes over MoQ; no full Blossom semantics.
- Small items: inline ciphertext in wrappers to avoid extra subscription.

## Error Handling & Churn
- Epoch churn: clients process commits from wrappers; after commit, switch to new `base` keys; briefly accept prior gen/epoch keys.
- Label rotation: re‑subscribe on directory updates; allow a grace period where both old and new labels may be active.
- Loss/reorder: nonce/gen cache covers out‑of‑order frames; MoQ partial reliability can drop late frames as needed.

## Interop
- Nostr-only clients: unchanged; wrappers are identical.
- MoQ clients: prefer MoQ for wrappers and media; dual-publish wrappers to Nostr if needed, or rely on a stateless bridge to mirror MoQ→Nostr.
- Privacy: single wrappers stream; no type split; random roots/labels; identity learned only upon decrypt.

## Transport Discovery
- Announce MoQ availability via a dedicated Marmot wrapper event distributed over the existing Nostr relay list; the chat bridge mirrors the latest offer onto the `marmot/<G>/wrappers` track for MoQ-native listeners.
- Only emit or replace the offer when the MoQ configuration changes (origin, root hint, ingest policy, capability scheme); routine MLS commits and epoch churn do not require a refresh.
- Capture this signalling in a forthcoming optional MIP so non-MoQ Marmot deployments can ignore it without impacting interoperability.
