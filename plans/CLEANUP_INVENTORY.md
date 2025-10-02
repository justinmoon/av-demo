# Cleanup Inventory - Before Next Phase

## Current Status
✅ **Working**: 3-participant E2EE text chat over MoQ with WebTransport
✅ **Tested**: Playwright test validates full flow with local relays
✅ **Manual**: dev-server.sh provides single-command setup

## Critical Issues to Fix Before Adding Features

### 1. ✅ **File Naming Disaster** (COMPLETED)
**Problem**: Arbitrary "part1/part2/part3" naming gives zero semantic meaning

**Files renamed** (commits bd6179e, f852059):
```
controller/state/part1.rs → core.rs (then split into types.rs, member.rs, message.rs, ready.rs)
controller/state/part2.rs → handshake.rs
wasm/part1a.rs → identity.rs
wasm/part1b.rs → nostr_client.rs
wasm/part2.rs → moq_bridge.rs
wasm/part3.rs → controller_bridge.rs
wasm/part4.rs → wrapper_utils.rs
```

**Additional refactor**: Split controller/state/core.rs (346 lines) into semantic modules:
- `types.rs` (70 lines) - type definitions
- `member.rs` (124 lines) - member management
- `message.rs` (59 lines) - message handling
- `ready.rs` (36 lines) - ready state management
- `core.rs` (82 lines) - constructor and basic helpers
- `handshake.rs` (unchanged) - handshake protocol
- `utils.rs` (41 lines) - utility functions

**Result**: Clear semantic organization, files under 150 lines each

---

### 2. **State Management Duplication** (spaghetti.md §Membership State)
**Problem**: Member state tracked in 4+ places, easy to desync

**Current**:
- `self.members: BTreeMap<String, MemberRecord>` (controller)
- `self.peer_pubkeys: BTreeSet<String>` (controller)
- `MemberRecord.joined` flag
- MDK/OpenMLS internal state (`identity.list_members()`)
- `self.subscribed_peers: BTreeSet<String>` (MOQ subscriptions)

**Symptoms**:
- `mark_member_joined()` and `sync_members_from_identity()` overlap
- No single source of truth
- Easy to get out of sync

**Fix**:
- Derive membership from MDK as single source of truth
- Keep minimal UI-facing cache only (roster display)
- Track MoQ subscription state separately (already done with `subscribed_peers`)
- Remove `members` HashMap and `peer_pubkeys` Set, use MDK queries

---

### 3. ✅ **include!() Anti-Pattern** (COMPLETED)
**Problem**: Files are `include!()` into parent instead of proper modules

**Fixed** (commits bd6179e, f852059):
- Replaced all `include!()` with proper `mod` declarations
- `controller/state/mod.rs` now uses standard module system
- `wasm/mod.rs` uses proper module declarations
- All modules properly scoped with `pub(super)` for internal APIs

**Result**: Standard Rust module organization, better IDE support, clear public APIs

---

### 4. **UI Layer Issues**

**File structure**:
```
apps/chat-ui/
├── src/
│   ├── bridge/moq.ts          # MoQ bridge (hardcoded "wrappers" track name)
│   ├── chat/controller.ts     # Controller integration
│   ├── ui/
│   │   ├── App.tsx
│   │   ├── ChatView.tsx
│   │   └── Onboarding.tsx
│   ├── types.ts
│   └── utils.ts
├── styles.css                 # Monolithic CSS (327 lines)
├── server.js                  # Dev server
└── dist/                      # Build output
```

**Issues**:
- Monolithic `styles.css` (no component-scoped styles)
- Hardcoded track names in `moq.ts` (`const TRACK_NAME = 'wrappers'`)
- No error boundaries or loading states
- Success/error messages use inline styling (just added `.form-success`/`.form-error`)

**Improvements needed**:
- Component-scoped CSS or CSS modules
- Configurable track names from session config
- Error boundaries for WASM failures
- Loading/suspense states for async operations

---

### 5. **MoQ Bridge Hardcoding**

**File**: `apps/chat-ui/src/bridge/moq.ts`

```typescript
const TRACK_NAME = 'wrappers';  // Hardcoded!
```

**Problem**: When we add audio/video tracks, we'll need multiple track subscriptions per session

**Fix**: Pass track names from session config, support multi-track subscriptions

---

### 6. **Test Coverage Gaps**

**Current**:
- ✅ `tests/manual-ui-flow.spec.js` - 2-participant Playwright test
- ✅ `tests/step4-chat.spec.js` - 3-participant Playwright test
- ❌ No unit tests for Rust controller logic
- ❌ No unit tests for WASM bindings
- ❌ No UI component tests

**Needed**:
- Rust unit tests for state machine transitions
- WASM binding smoke tests
- UI component tests (Vitest + Testing Library)

---

### 7. **Documentation Gaps**

**What exists**:
- `plans/spaghetti.md` - Known tech debt (this inventory supersedes it)
- `plans/PHASE_1.md` - Text over MoQ plan (steps 1-6)
- `plans/MOQ_MARMOT_AV_PLAN.md` - Full A/V roadmap
- `CLAUDE.md` - Agent instructions
- `README.md` - Outdated

**What's missing**:
- Architecture overview (how controller/WASM/UI fit together)
- API documentation for WASM bindings
- Local development setup (beyond dev-server.sh)
- Handshake flow diagram (creator vs invitee paths)

---

### 8. **Build System Issues**

**Current**:
```json
"scripts": {
  "build:wasm": "wasm-pack build crates/marmot-chat --target web --out-dir ../../tests/pkg",
  "build:ui": "tsc && node apps/chat-ui/esbuild.config.mjs && cp tests/pkg/marmot_chat_bg.wasm apps/chat-ui/dist/",
  "build": "npm run build:wasm && npm run build:ui"
}
```

**Issues**:
- WASM output goes to `tests/pkg` then copied to `apps/chat-ui/dist`
- No incremental builds
- No watch mode for development
- TypeScript errors not breaking CI

**Fix**:
- WASM output directly to shared location
- Add watch mode scripts
- Proper CI validation

---

### 9. **State Machine Clarity** (spaghetti.md §State Machine Phases)

**Problem**: Handshake, ready, and connection states are intertwined

**Current**:
- `HandshakeState` enum (Idle/AwaitingWelcome/Established)
- `ready` boolean flag
- Multiple `ConnectMoq` calls

**Confusion**:
- When is it safe to publish?
- What does "ready" mean (handshake? MoQ? both?)
- Track readiness vs publish readiness

**Fix**:
- Single state machine: `Initial → Handshaking → Connected → Ready`
- Clear transitions and invariants
- Document when publishing is safe

---

### 10. **Error Handling**

**Rust**: Errors are logged but not always surfaced to UI
**UI**: Console errors, no user-facing error recovery

**Example**: 3rd participant joining sees cryptographic errors during sync (technically fine, but scary)

**Improvements**:
- Transient errors during handshake should be hidden
- Critical errors should show actionable UI
- Retry logic for transient failures

---

## Prioritized Cleanup Tasks

### P0 (Do Before Next Feature)
1. ✅ **Rename part*.rs files** - DONE (commits bd6179e, f852059)
2. **State management cleanup** - Remove duplicate member tracking
3. **Document architecture** - How pieces fit together

### P1 (Do This Week)
4. ✅ **Fix include!() pattern** - DONE (commits bd6179e, f852059)
5. **Add unit tests** - Controller state transitions
6. **Error boundary UI** - User-facing error recovery

### P2 (Nice to Have)
7. **Component CSS** - Scope styles properly
8. **Build system** - Watch mode, incremental builds
9. **State machine clarity** - Document transitions

---

### 11. **Operation Queue Pattern** (spaghetti.md §Async/Event Loop)

**Problem**: Operations queued in `run_event_loop()` can trigger other operations

**Current**:
- `schedule()` helper adds operations to queue
- Operations can chain (e.g., `ConnectMoq` → `Ready` event)
- Not clear what's sync vs async

**Fix**:
- Document operation dependencies and execution order
- Consider explicit state machine with transitions
- Or use actor model with clear message passing

---

### 12. **Testing Lessons Learned**

**NEVER USE MOCKS FOR INTEGRATION LOGIC**

**What happened**: LocalMoqService mock hid critical MoQ subscription bug
- Mock didn't require subscriptions → code "worked"
- Real MoQ requires explicit subscribe → Alice never subscribed to Carol's track
- Bug only found when switching to real relay

**Current approach** (✅ FIXED):
- Removed all mocks from integration paths
- Playwright tests use real moq-relay + nostr-relay
- Unit tests only for pure functions (crypto, parsing, state transitions)

**Rule**: If it talks to external systems, test with real infrastructure

---

## What's Actually Working Well

✅ **MoQ integration** - WebTransport works, relay flags correct
✅ **MLS/MDK** - Encryption, epochs, welcomes all work
✅ **Nostr handshake** - Key package exchange solid
✅ **Multi-participant** - 3+ participants validated
✅ **Dev workflow** - Single command to run everything

---

## Next Steps

1. **Review this inventory** with team
2. **Create refactor plan** - Which P0 tasks to tackle first
3. **Don't add features** until file naming and state management are fixed
4. **Write tests** as we refactor (no untested refactors)

---

## References

- `plans/spaghetti.md` - Original tech debt doc (merge this content there)
- `plans/PHASE_1.md` - Where we are in the plan (Step 4 complete)
- `plans/MOQ_MARMOT_AV_PLAN.md` - Next: Phase 2 (Audio)
