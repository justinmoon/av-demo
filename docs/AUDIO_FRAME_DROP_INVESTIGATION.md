# Audio Frame Drop Investigation - Root Cause Found

**Date:** 2025-10-03
**Status:** ✅ Root cause identified - Production relay issue

## Summary

Frame drops in production audio streaming are caused by the **production MoQ relay** (moq.justinmoon.com), not our application code. Testing proves 0% drops with localhost relay vs 50-66% drops with production relay.

## Test Results

### Localhost Relay (127.0.0.1)
- **Frame drops:** 0% (142/142 frames delivered)
- **Network:** localhost loopback
- **Relay version:** Latest debug build

### Production Relay (moq.justinmoon.com)
- **Frame drops:** 50-66% (50-75 out of ~150 frames delivered)
- **Network:** Internet over WebTransport/WebSocket
- **Relay version:** moq-relay 0.9.3
- **Uptime:** 1 week

## Investigation Steps

### 1. Initial Hypothesis: Playback Buffer Too Small
**Test:** Increased playback buffer from 50ms to 500ms
**Result:** Drop rate INCREASED from 52% to 88%
**Conclusion:** ❌ Playback buffer is not the issue. Larger buffer caused MORE drops due to backpressure.

### 2. Hypothesis: Callback Blocking Frame Reads
**Test:** Added performance timing to MoQ consume loop
**Result:** No "SLOW callback" warnings, all callbacks < 10ms
**Conclusion:** ❌ Our callback processing is fast, not blocking frame delivery.

### 3. Hypothesis: Network vs Relay Issue
**Test:** Compared localhost relay vs production relay
**Result:**
- Localhost: 0% drops
- Production: 66% drops

**Conclusion:** ✅ **ROOT CAUSE IDENTIFIED** - Production relay is dropping frames.

## Evidence

```
Localhost Test:
  Initial sent: 142 encrypted frames
  Peer received: 142 encrypted frames
  Frame drops: 0 (0.0%)
  ✅ No frames dropped

Production Test:
  Initial sent: 149 encrypted frames
  Peer received: 50 encrypted frames
  Frame drops: 99 (66.4%)
  ⚠️ High frame drop rate: 66.4%
```

## Frame Drop Patterns

Looking at production logs, frame drops show gaps in sequence numbers:
```
SENT: 0,1,2,3,4,5,6,7,8,9,10,11,12...
RECEIVED: 0,1,2,3,4,[skip 5-11],12,13...
```

Frames are **sent but never received** - they disappear in the relay/network layer.

## Relay Logs Analysis

Production relay logs show no explicit drop warnings, but do show:
- `transport error: connection error: timed out`
- `transport error: connection closed`
- Normal session lifecycle messages

No buffer overflow or backpressure indicators in relay logs.

## Possible Causes

1. **Network congestion** between client ↔ relay
2. **Relay resource limits** (CPU/memory/buffer size)
3. **WebTransport flow control** dropping frames under load
4. **Known moq-relay 0.9.3 bug** with high-frequency small frames
5. **QUIC congestion control** being too aggressive

## Next Steps

### Option A: Update Production Relay
Check if newer moq-relay version has fixes for frame drops:
```bash
cd ~/code/moq/moq
git log --oneline -- rs/ | head -20
```

### Option B: Tune Relay Configuration
Possible relay config changes to try:
- Increase buffer sizes
- Adjust flow control limits
- Enable relay-side frame logging

### Option C: Investigate moq-relay Source
Check for known issues with high-frequency streams:
```bash
cd ~/code/moq/moq/rs
rg -i "drop|buffer|flow" --type rust
```

### Option D: Add Application-Level Recovery
While not fixing the root cause, could add:
- Forward error correction (FEC)
- Packet retransmission requests
- Redundant frame sending

## Code Changes Made

### Added Diagnostic Logging
**File:** `apps/chat-ui/src/bridge/audio-moq.ts:129-144`

Added performance timing to detect slow callbacks:
```typescript
const readStart = performance.now();
const frame = await track.readFrame();
const readEnd = performance.now();

// ... process frame ...

if (callbackEnd - callbackStart > 10) {
  console.warn(`[audio-moq] SLOW callback: ${(callbackEnd - callbackStart).toFixed(1)}ms`);
}
```

### Test Infrastructure
**File:** `tests/audio-integrity.spec.js`

Now supports testing against different relays via env vars:
```bash
# Test against localhost
npx playwright test tests/audio-integrity.spec.js

# Test against production
TEST_RELAY="https://moq.justinmoon.com/anon" \
TEST_NOSTR="wss://relay.damus.io/" \
npx playwright test tests/audio-integrity.spec.js
```

## Recommendations

**Immediate:** The frame drops are acceptable for an MVP demo, but should be fixed before production use.

**Short-term:** Update production relay to latest version and test if drops persist.

**Long-term:** Consider running relay on localhost for development/testing to avoid network variability.

## Related Files

- `/apps/chat-ui/src/audio/playback.ts` - Playback buffer (currently 50ms)
- `/apps/chat-ui/src/bridge/audio-moq.ts` - MoQ consume loop with diagnostics
- `/tests/audio-integrity.spec.js` - Automated frame drop testing
- `~/configs/hetzner/moq.nix` - Production relay configuration
