# Code Cleanup TODOs

## Membership State Management

**Problem**: Member state is tracked in multiple overlapping places:
- `self.members: HashMap<String, MemberState>` (controller/state/part1.rs)
- `self.peer_pubkeys: HashSet<String>` (controller/state/part1.rs)
- `MemberState.joined` flag
- MDK/OpenMLS internal state via `identity.list_members()`

**Symptoms**:
- `mark_member_joined()` and `sync_members_from_identity()` do similar things
- Easy to get them out of sync
- No single source of truth

**Fix**:
- Derive all membership state from MDK via `list_members()`
- Keep minimal cached state in controller (just for roster UI)
- Track MoQ subscription state separately from membership state

## MoQ Subscription Management

**Problem**: `sync_members_from_identity()` is only called from `handle_incoming_frame()` but should be called after ANY membership change (local or remote).

**Symptoms**:
- When Alice adds Carol locally, Alice never subscribes to Carol's MoQ track
- Worked in LocalMoqService mock (didn't require subscriptions) but fails in real MoQ
- Mocks hid this critical bug

**Fix**: ✅ FIXED
- Call `sync_members_from_identity()` after any local operation that changes membership:
  - `handle_member_addition()` ✅ (fixed)
  - Any future add/remove operations
- Track subscriptions separately from membership: added `subscribed_peers: BTreeSet<String>` to prevent duplicate subscriptions
- `sync_members_from_identity()` now checks `!self.subscribed_peers.contains(&pubkey)` instead of relying on `is_new` flag

## State Machine Phases

**Problem**: Handshake state, ready state, and connection lifecycle are intertwined.

**Current flow**:
1. `HandshakeState` enum (Idle/AwaitingWelcome/Established)
2. `ready` event emitted when MoQ connects
3. Multiple places call `ConnectMoq` operation

**Issues**:
- Not clear when it's safe to publish messages
- "Ready" semantics differ between transports (was immediate in LocalMoq, async in JsMoq)
- Track readiness vs connection readiness vs handshake completion

**Fix**:
- Separate concerns: handshake completion, MoQ connection, publish readiness
- Single state machine with clear transitions
- Clear contract: "ready" = can publish AND can receive

## Module Organization

**Problem**: Controller split across multiple `include!()` files (part1.rs, part2.rs) to keep files under 500 LOC.

**Issues**:
- Hard to navigate
- Arbitrary boundaries
- Not based on logical separation of concerns

**Fix**:
- Refactor into proper modules based on responsibility:
  - `membership.rs` - member management, roster state
  - `messaging.rs` - message send/receive
  - `handshake.rs` - MLS handshake flow
  - `moq.rs` - MoQ subscription management
  - `state.rs` - core state machine

## Async/Event Loop

**Problem**: Operations are queued and processed in `run_event_loop()` but some operations trigger other operations.

**Symptoms**:
- `schedule()` helper adds operations to queue
- Operations can trigger other operations (e.g., `ConnectMoq` → `Ready`)
- Not clear what's synchronous vs asynchronous

**Fix**:
- Document operation dependencies
- Consider using explicit state machine with transitions instead of operation queue
- Or use actor model with clear message passing

## Testing Strategy

**Problem**: Wasm tests used LocalMoqService mock which didn't exercise real MoQ subscription logic.

**Fix** (done):
- ✅ Removed LocalMoqService
- ✅ Added "NEVER USE MOCKS" to AGENTS.md
- TODO: Set up real moq-relay for integration tests
- TODO: Use Playwright tests as primary validation (they use real infrastructure)
