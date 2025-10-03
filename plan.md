Project goal: bring Phase‑2 encrypted audio to the MoQ + Marmot A/V demo. Phase 1
(text
control plane) already works. We now need true MLS-derived Opus encryption over MoQ
tracks
with browser playback.

Key docs:

- plans/MOQ_MARMOT_AV_PLAN.md (Phase 2 tasks).
- plans/MOQ_MARMOT_AV_SPEC.md (directory schema, key schedule, AEAD).
- plans/MOQ_CHAT_SERVER.md, plans/NOSTR_AUTH.md for auth/relay context.
- Look at sibling projects for inspiration:
  - ~/code/moq/orange (MLS worker + encrypted WebRTC pipeline).
  - ~/code/moq/moq/demos (Hang-based MoQ meeting transport in TypeScript).
  - ~/code/moq/innpub (audio capture/playback helpers, worklets).
  - ~/code/moq/moq/rs/hang (Rust hang library for integration tests).

High-level checkpoints (pause after each for manual validation):

1. MoQ path alignment & plumbing check - Derive the MoQ root from the MLS exporter (IdentityHandle::derive_group_root)
   and
   feed it into the WASM ↔ JS bridge so we stop using the ad-hoc sessionId. - Update onboarding/session handling accordingly. - Add a note to plans/spaghetti.md describing the before/after. - ➜ Pause, summarize the changes, wait for feedback.
2. Encrypted directory groundwork - Add Rust structs to (de)serialize the directory application message, emit it
   when
   local tracks change or epochs rotate, and handle ingestion on receive. - Plumb directory events through ChatEvent so the UI can react. - Write Rust tests for parse/serialize. - ➜ Pause with a short validation plan + test output.
3. AES/AEAD key schedule module - Implement the exporter-based media key derivation (HKDF per generation, nonce
   mapping, AAD layout) in Rust. - Provide encrypt/decrypt helpers callable from WASM. - Cover with unit tests (encrypt→decrypt, generation rollover, cache). - ➜ Pause, show test results and API surface.
4. Rust-only streaming integration test demo - Using moq/rs/hang or moq-lite, script a two-member MLS group where one sender
   streams an Opus (or PCM) file, encrypts frames with the new helper, and the receiver
   decrypts. - Keep this as an integration test/binary under our repo (no external clones). - ➜ Pause with test instructions and results.
5. JS bridge upgrade (cleartext first) - Refactor apps/chat-ui/src/bridge/moq.ts (or split into a new module) to support
   multiple tracks using @kixelated/moq or @kixelated/hang. - Implement publish/subscribe for audio labels, using cleartext frames to start. - Provide a simple UI toggle to enable mic capture (can leverage innpub’s
   helpers). - ➜ Pause with manual testing steps before adding encryption.
6. Wire encryption in the browser loop - Call the WASM encrypt/decrypt helpers from the JS audio pipeline (publish &
   subscribe). - Ensure directory updates trigger subscriptions, and generation/epoch churn is
   respected. - Expand tests (Rust + Playwright) to cover the encrypted path or explain manual
   verification steps. - ➜ Pause with status + remaining gaps.
7. E2E validation & cleanup
   - Run just ci.

- Update docs if behavior changed (README.md, spec snippets).
- File any follow-up items.
- Final pause for approval.

Important operating rules:

- After each numbered step, STOP and wait for explicit go-ahead. Provide a concise
  summary, relevant diffs, and test output. No jumping ahead.
- No fake stubs: if you introduce APIs, they must work end-to-end.
- Prefer @kixelated/hang for JS track management and reuse innpub audio helpers when
  possible—don’t re-implement without reason.
- Keep notes on technical debt in plans/spaghetti.md.
- If you need to browse other repos, you may read them but do not modify external
  directories.
- All commands must include workdir; don’t use cd.
- The environment uses zsh, sandbox-free, no approvals. Use real tests (no mocks).

Let me review after every checkpoint before you continue.
