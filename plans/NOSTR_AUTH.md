Nostr-Based Auth for MoQ (JWT Alternative)

Overview
- Goal: remove the centralized JWT issuer by letting users self‑authorize access to MoQ paths, and optionally use one‑time preimage proofs for anonymous write access.
- Scope: keeps moq-relay stateless, requires no Nostr queries at runtime, and maps directly to existing path‑scoped publish/subscribe semantics.
- Transport: use URL query parameters (WebTransport limitation). Later, move to an Authentication header or handshake frame.

Core Ideas
- Self‑issued capability: a signed, short‑lived token stating the path root and allowed get/put scopes. Verifiable by any relay without a shared secret.
- Pubkey‑hash write proof: minimal, gateway‑free proof that the connector controls the key that owns a specific write label.
- Optional preimage variants: hash‑lock/chain/Merkle one‑time authorizations with zero public‑key exposure.

Path Model (unchanged)
- The effective authorization in moq-relay remains: a connection URL path contains a root prefix; allowed publish/subscribe subpaths are computed relative to that root.
- The new auth methods must yield an `AuthToken { root, subscribe[], publish[], cluster=false }` equivalent to today’s JWT flow.

Transport Fields
- Self‑issued capability: `?cap=<base64url-canonical-json>&sig=<hex-schnorr>`
- Pubkey‑hash write proof: `?pk=<hex32>&sig=<hex>&ts=<unix>&nonce=<hex>`
- Preimage (hash‑lock): `?preimage=<hex32>&ts=<unix>&nonce=<hex>`

Phase 1 — Self‑Issued Capabilities
- Purpose: full replacement for centralized JWTs with at least the same flexibility.
- Payload (canonical JSON; JCS recommended):
  {
    "ver": 1,                  // format version
    "kid": "<hex32|npub>",   // signing key identifier (secp256k1)
    "root": "<path-root>",   // required path root prefix (e.g., "hash/<hex>", "pk/<npub>")
    "get": ["..."],           // allowed subscribe scopes relative to root; empty = no reads
    "put": ["..."],           // allowed publish scopes relative to root; empty = no writes
    "exp": 1703980800,         // expiration (unix seconds)
    "nbf": 1703977200,         // not‑before (optional)
    "aud": ["relay.example"], // optional audience restriction (hostnames)
    "jti": "<random>"         // optional id for replay caches
  }
- Signature
  - Message: `sha256(UTF-8-bytes-of-canonical-json)`
  - Algorithm: BIP‑340 Schnorr over secp256k1.
  - URL: `.../path?cap=<base64url(json)>&sig=<hex>`
- Verification (relay)
  1) Parse `cap` and `sig`. Re‑encode payload canonically, compute `msg=sha256(payload)`.
  2) Parse `kid` to secp256k1 key; verify Schnorr(sig, msg, kid).
  3) Check `exp/nbf` against wall clock (with small skew tolerance).
  4) If `aud` present, require `request.host ∈ aud`.
  5) Path containment: let `url_path = request.url().path()`; require `url_path` contains `payload.root` as prefix; compute `suffix = url_path.strip_prefix(root)`.
  6) Build `subscribe[]` and `publish[]` from `get[]`/`put[]` exactly like JWTs today ("" means unrestricted under the root, non‑empty entries get joined relative to `suffix`).
  7) Return `AuthToken { root, subscribe, publish, cluster: false }`.
- Namespace privacy
  - Hide identity by using hashed roots: `root = "hash/" + sha256(pubkey)` (or host‑bound hash).
  - For stronger unlinkability, use an ephemeral key to sign the capability and root.

Phase 2 — Pubkey‑Hash Write Proof (Minimal, No Token)
- Purpose: gateway‑free proof for write access under an identity‑bound label; ideal for ingest paths.
- Path convention: `/ingest/<label>/...` where `<label> = sha256(pubkey)` (truncate if desired).
- Client query params
  - `pk`: hex 32‑byte pubkey (x‑only)
  - `ts`: unix seconds (string/int)
  - `nonce`: random hex (≥ 8 bytes)
  - `sig`: hex 64‑byte Schnorr signature over:
    - Canonical string: `"moq-write-v1\nhost:" + host + "\npath:" + path + "\nts:" + ts + "\nnonce:" + nonce`
    - Message: `sha256(canonical-string)`
- Verification (relay)
  1) Rebuild canonical string with the actual `host` and `path` from the incoming request.
  2) Verify Schnorr(sig, msg, pk).
  3) Require `abs(now - ts) <= skew_window` (e.g., 120s).
  4) Optionally LRU cache `(pk, nonce)` to prevent immediate replay.
  5) Require `label == sha256(pk)` for the `/ingest/<label>/...` segment.
  6) Produce write‑only `AuthToken` with `publish=[""]` under the connection root and `subscribe=[]`.

Optional — Secret Preimage Variants (Anonymous or One‑Time)
- Hash‑lock (single‑use): root is `hash = sha256(s)`; client presents `s, ts, nonce`; relay verifies `sha256(s)==hash` and marks consumed. Bind roots per host (e.g., `sha256(host || s)`) to avoid cross‑relay reuse.
- Hash‑chain (one‑time passwords): store current tip `R = H^N(s0)` as path label; client reveals `si` such that `H(si)==R`; relay updates `R=si`. One‑time by construction; no npub or long‑term secret at relay.
- Merkle batch: path label is Merkle root; client proves inclusion with one leaf preimage and a proof; relay marks leaf as used.
- Use cases: anonymous dropbox‑style ingest; for rich policies, prefer self‑issued capabilities.

Integration: moq-relay Changes
- `moq/rs/moq-relay/src/connection.rs:19`
  - Parse additional query params alongside `?jwt=`:
    - Self‑cap: `cap`, `sig`
    - Write‑proof: `pk`, `sig`, `ts`, `nonce`
    - Preimage (optional): `preimage`, `ts`, `nonce`
- `moq/rs/moq-relay/src/auth.rs`
  - Add verification helpers that return `AuthToken`:
    - `verify_cap(path, cap_bytes, sig)`
    - `verify_write_proof(path, pk, sig, ts, nonce)`
    - `verify_preimage(path, preimage, ts, nonce)` (optional)
  - Dispatcher in `verify(...)` tries, in order: JWT → self‑cap → write‑proof → public.
  - Preserve existing path joining and scoping logic to build `subscribe[]`/`publish[]`.
- Config (optional)
  - Extend `AuthConfig` to enable/disable new modes and set ingest prefix (`ingest/`) and skew/replay windows.
- Cluster
  - No change required. Cluster auth can remain JWT‑based; or, when hopping, forward the same `?cap` intact if desired. This can be decided per deployment.
- Docs
  - Add a section to `moq/docs/auth.md` describing Self‑Issued Capabilities and Write Proofs with examples.

Examples
- Self‑issued capability URL
  - `https://relay.example.com/hash/3f..ab/room1?cap=eyJ2ZXIiOjEsICJraWQiOiAi..."&sig=ab12...`
  - Payload: `{ "ver":1, "kid":"<hex32>", "root":"hash/3f..ab", "get":["wrappers","blob"], "put":["ingest/*"], "exp":1703980800 }`
- Write‑proof URL
  - `https://relay.example.com/ingest/3f..ab/cam?pk=<hex32>&ts=1703977200&nonce=9f3d...&sig=6a...`

Security Considerations
- Canonicalization: use JCS (RFC 8785) for JSON payload to avoid signature ambiguities.
- Audience binding: include `aud[]` in capabilities; in write‑proof, bind to host in the canonical string.
- Time windows: enforce `exp/nbf` (capabilities) and `ts` skew (write‑proof).
- Replay: use `jti` (capabilities) and `(pk,nonce)` LRU (write‑proof); preimage schemes must track “used”.
- Privacy: avoid logging query params; prefer hashed or random roots; ephemeral keys reduce linkability.
- Transport: all auth flows rely on TLS; move to headers/handshake once available.

Shortcomings & Open Questions
- Canonical JSON vs CBOR: CBOR (deterministic) is smaller but adds complexity; JSON+JCS is simpler to start.
- Delegation chains: define an embedded `proofs[]` field (UCAN‑style) for granting others scoped rights; or reference a Nostr policy event ID. Verification remains local if proofs are embedded.
- Revocation: short TTLs and rotating roots mitigate; explore revocation lists via Nostr for longer‑lived caps.
- Multi‑relay audiences: clarify semantics when `aud[]` is omitted vs multiple hostnames present.
- Cluster auth: whether to forward user caps upstream or keep separate cluster tokens.
- Server challenge: WebTransport limits header interactivity; current design uses client‑supplied `ts/nonce` instead.

Alternatives
- Asymmetric JWT/JWS: switch moq‑relay to accept ECDSA/Schnorr‑signed JWTs (no shared secret) and keep existing plumbing.
- UCAN: adopt UCAN cap format directly for self‑issued, delegated capabilities.
- Macaroons/LSAT: per‑relay minted tokens for rate limits/pay‑to‑use layered atop self‑issued caps.

Implementation Notes (quick map)
- Replace central JWT authorization by adding two verifiers and a dispatcher; keep existing `AuthToken` plumbing.
- Reuse Schnorr verification code patterns from `nostr-moq/gateway` for consistency.
- Start with JSON+JCS and URL params; migrate transport later as infra permits.

