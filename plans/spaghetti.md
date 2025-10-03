# Controller State Cleanup Notes
- Roster emission now hits `identity.list_members()` on every event (joins/admin updates). If this ever shows up in profiling, consider a lightweight cache keyed by epoch with invalidation hooks from MDK.
- `notify_new_member` triggers a roster emit per new peer; batching roster updates after multi-member commits might reduce duplicate UI refresh work.
- Align `scripts/dev.sh` defaults with UI defaults (nostr now 8880) and fail fast if the port is busy so manual smoke tests don't silently misconfigure relays again.
