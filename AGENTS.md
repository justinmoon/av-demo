# Agents Guide

Our goal here is to build an e2ee video and audio calling app on top of Marmot (formerly Whitenoise) protocol -- which is an MLS-based E2EE text chat spec for Nostr -- using MOQ for transport. This is an MVP so we want to keep it simple, we want to max out on privacy, and make it as fast as we can by leverating MOQ's capabilities.

- Primary docs:
  - plans/MOQ_MARMOT_AV_PLAN.md — phased plan
  - MOQ_MARMOT_AV_SPEC.md — protocol/spec details (auth, directory, AEAD)
  - MOQ_CHAT_SERVER.md — MoQ chat accelerator (events track, ingest, blobs)
  - NOSTR_AUTH.md — self‑issued caps + write‑proof auth

We have many related projects in ~/code/moq that you can look at for references or ideas. feel free to checkout more. If you need to fork dependencies, just make a branch in a checkout here and get it working with a local dep. Then push to a fork on justinmoon github get it working with a local dep. Then push to a fork on justinmoon github user using `gh`.

Keep things simple. Try to keep directory structure reasonably flat.

Never stub things out for a real implementation later unless you are explicitely told to do so. Your job is to make a real implementation now.
