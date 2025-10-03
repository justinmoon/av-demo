# Controller State Cleanup Notes
- Roster emission now hits `identity.list_members()` on every event (joins/admin updates). If this ever shows up in profiling, consider a lightweight cache keyed by epoch with invalidation hooks from MDK.
- `notify_new_member` triggers a roster emit per new peer; batching roster updates after multi-member commits might reduce duplicate UI refresh work.
- Align `scripts/dev.sh` defaults with UI defaults (nostr now 8880) and fail fast if the port is busy so manual smoke tests don't silently misconfigure relays again.

## MoQ Path Alignment (Phase 2 checkpoint 1)

**Before:**
- Onboarding generated a random UUID `sessionId` (Onboarding.tsx:112)
- Both Nostr handshake channel and MoQ transport used this ad-hoc session ID
- No cryptographic binding between MLS group and MoQ transport paths

**After:**
- `sessionId` still used for Nostr handshake bootstrap channel (required for initial key exchange)
- After MLS group establishment, derive MoQ root via `IdentityHandle::derive_group_root()`:
  - Uses stable MLS `group_id` (not epoch-specific export_secret)
  - Returns `"marmot/{hex}"` format where hex is the group_id (services.rs:317-322)
  - Stored in `SessionParams.moq_root` (events.rs:65)
  - **Critical**: Uses group_id not export_secret to ensure all members use the same path regardless of epoch
- MoQ connect uses `moq_root` when available, falls back to `session_id` (mod.rs:136-140)
- Derivation happens in both creator and invitee paths after handshake establishment (handshake.rs:287-290, 368-371)

**Benefits:**
- MoQ transport path is now cryptographically derived from MLS group identity
- Stable across epoch changes (all members at any epoch use the same path)
- Provides foundation for Phase 2 encrypted media (group_id + export_secret for key derivation)
- Backward compatible via fallback (handles sessions where group hasn't been established yet)

**Potential follow-ups:**
- Consider removing session_id fallback once bootstrap flow is guaranteed to establish group first
- UI could expose the derived moq_root for debugging/verification
- When implementing Phase 2 media encryption, use export_secret for per-epoch key derivation (not group_id)
