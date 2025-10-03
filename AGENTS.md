# Agents Guide

Think about how we can improve our spaghetti code as you work. Opportunistically take notes in plans/spaghetti.md. In the future we'll do a pass where we clean these things up.

NO MOCKS. NO FAKE CODE.

Our goal here is to build an e2ee video and audio calling app on top of Marmot (formerly Whitenoise) protocol -- which is an MLS-based E2EE text chat spec for Nostr -- using MOQ for transport. This is an MVP so we want to keep it simple, we want to max out on privacy, and make it as fast as we can by leverating MOQ's capabilities.

- Primary docs:
  - plans/MOQ_MARMOT_AV_PLAN.md — phased plan
  - MOQ_MARMOT_AV_SPEC.md — protocol/spec details (auth, directory, AEAD)
  - MOQ_CHAT_SERVER.md — MoQ chat accelerator (events track, ingest, blobs)
  - NOSTR_AUTH.md — self‑issued caps + write‑proof auth

We have many related projects in ~/code/moq that you can look at for references or ideas. feel free to checkout more. If you need to fork dependencies, just make a branch in a checkout here and get it working with a local dep. Then push to a fork on justinmoon github get it working with a local dep. Then push to a fork on justinmoon github user using `gh`.

Keep things simple. Try to keep directory structure reasonably flat.

Never stub things out for a real implementation later unless you are explicitely told to do so. Your job is to make a real implementation now.

## Testing Philosophy

**NEVER USE MOCKS.** Mocks hide bugs by not exercising real code paths. Use real infrastructure (moq-relay, real network, real async) even in tests. The LocalMoqService mock hid critical bugs where participants didn't subscribe to each other's MoQ tracks properly - tests passed but production failed.

## CI Requirements

**`just ci` MUST PASS** before any feature or change is considered complete. The agent must iterate on the code and get all CI checks passing. This includes:
- Cargo fmt check
- Cargo clippy (with -D warnings)
- Rust unit tests
- WASM tests
- TypeScript type checking
- Playwright tests
