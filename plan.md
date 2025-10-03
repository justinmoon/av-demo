# Audio Quality Improvement Plan

Based on code review findings and AUDIO_QUALITY_RESEARCH.md, this plan systematically fixes audio quality issues by mining proven solutions from sibling branches.

## Current State

- ✅ Encrypted audio works end-to-end over MoQ
- ❌ Frame skipping causes choppy audio (gaps of exactly 1 frame)
- ❌ Using wrong chunk size (1024 samples = 21.33ms instead of 960 = 20ms)
- ❌ Using deprecated ScriptProcessorNode instead of AudioWorklet
- ❌ No jitter buffer for playback
- ❌ No latency compensation

## Sibling Branches to Mine

- `audio-encryption-moq-claude/` - AudioWorklet, Opus codec, test hooks
- `audio-encryption-moq-codex/` - Audio engine, ring buffer, MoQ bridge
- `mls-opus-encryption-codex/` - Full audio engine with buffering

## Implementation Phases

### Phase 0: Establish Baseline ✅ COMPLETE

**Goal:** Get audio-integrity.spec.js working to measure improvements

1. ✅ Create audio-integrity.spec.js with real audio file using Chromium flags
2. ✅ Test validates: no frame drops, audio integrity, encryption works
3. ✅ Run `just ci` + all Playwright tests - **ALL PASSING**
4. ✅ Document baseline findings

**Baseline Results (3 seconds of high-quality 48kHz audio):**
- **Initial sent**: 144 encrypted frames
- **Peer received**: 144 encrypted frames
- **Frame drops**: 0 (0.0%) ✅
- **Current chunk size**: 1024 samples (21.33ms @ 48kHz) ≈ 46.875 fps
- **Expected frames**: ~141 frames (144,000 samples ÷ 1024)
- **All CI checks**: ✅ PASSED (1 skipped, 7 passed)

**Key Observations:**
- Audio encryption/decryption works end-to-end over realistic duration
- No frame drops in controlled test environment (144/144 = 100% delivery)
- `window.audioStats` provides frame counters for testing
- Test runs fast (~4.1s) and reliably
- Frame rate math: 144 frames / 3 seconds ≈ 48 fps (close to expected 46.875)

**Next Step:** Apply Phase 1 fixes (chunk size 960 + latency buffer) to see if it improves real-world usage

---

### Phase 1: Quick Wins - Chunk Size + Latency Buffer

**Goal:** Fix timing issues with minimal code changes

**Changes:**
1. Set chunk size from 1024 → 960 samples (20ms exactly @ 48kHz)
   - File: `apps/chat-ui/src/ui/ChatView.tsx:416`
2. Add 50ms playback latency buffer
   - File: `apps/chat-ui/src/audio/playback.ts:32`
   - Change: `const startTime = Math.max(now + 0.05, nextStartTime);`
3. Verify sample rate is 48000, log mismatches

**Validation:**
- Run audio-integrity.spec.js - expect fewer/no frame drops
- Check frame skip warnings in console
- **Pause for approval before Phase 2**

---

### Phase 2: AudioWorklet Migration

**Goal:** Replace ScriptProcessorNode with AudioWorklet for better timing

**Source:** `audio-encryption-moq-claude/apps/chat-ui/src/audio/`

**Changes:**
1. Port AudioWorklet capture
   - Source: `audio-encryption-moq-claude/apps/chat-ui/src/audio/capture.ts:30`
   - Worklet: `audio-encryption-moq-claude/apps/chat-ui/src/audio/worklets/capture-worklet.js:1`
2. Add RMS level reporting for monitoring
3. Use transferable objects for zero-copy message passing
4. Update ChatView to use new AudioWorklet API

**Validation:**
- Run audio tests - timing should be more precise
- Check RMS levels are reported
- **Pause for approval before Phase 3**

---

### Phase 3: Playback Buffering & Ring Buffer

**Goal:** Smooth playback despite network jitter

**Source:** `mls-opus-encryption-codex/apps/chat-ui/src/audio/engine.ts:120-180` or hang/innpub

**Changes:**
1. Implement ring buffer in `apps/chat-ui/src/audio/playback.ts:20`
2. Handle out-of-order frames gracefully
3. Fill gaps with silence instead of glitching
4. Track underflow/overflow for debugging
5. Add frame timestamps based on sample count

**Validation:**
- Test with artificial network jitter
- Verify graceful handling of dropped frames
- **Pause for approval before Phase 4**

---

### Phase 4: Opus Codec Integration

**Goal:** Use real Opus payloads instead of raw PCM

**Source:** `audio-encryption-moq-claude/apps/chat-ui/src/audio/opus.ts:14`

**Changes:**
1. Integrate WebCodecs Opus encoder/decoder
2. Maintain frame counter prefix in encrypted payload (apps/chat-ui/src/ui/ChatView.tsx:382)
3. Align frame sizes with Opus spec (10/20/40/60ms options)
4. Update encryption to handle Opus packets

**Validation:**
- Verify Opus encode/decode works
- Check bandwidth reduction vs PCM
- Ensure frame counters still track correctly
- **Pause for approval before Phase 5**

---

### Phase 5: MoQ Track Management Refactor

**Goal:** Use directory-driven track discovery instead of hardcoded labels

**Source:** `mls-opus-encryption-codex/apps/chat-ui/src/bridge/moq.ts:1`

**Changes:**
1. Refactor `apps/chat-ui/src/bridge/audio-moq.ts:16` to use directory announcements
2. Remove hardcoded `audio-${pubkey.slice(0,8)}` from `apps/chat-ui/src/bridge/audio-moq.ts:67`
3. Expose createAudioPublisher/subscribeToAudio API
4. React to directory updates in Solid state
5. Check if Rust needs updates (crates/marmot-chat/src/controller/events.rs:1)

**Validation:**
- Test track discovery with multiple peers
- Verify dynamic subscription to new audio tracks
- **Pause for approval before Phase 6**

---

### Phase 6: Test Instrumentation

**Goal:** Automated frame tracking without mocks

**Source:** `audio-encryption-moq-claude/apps/chat-ui/src/audio/test-hooks.ts:1`

**Changes:**
1. Port test instrumentation hooks
2. Allow Playwright to audit captured/encrypted/decrypted frames
3. Update audio-integrity.spec.js to use hooks
4. Add frame counter assertions
5. Track encryption/decryption statistics

**Validation:**
- All Playwright audio tests pass reliably
- Frame tracking works without mocks
- **Pause for approval before Phase 7**

---

### Phase 7: Packet Structure (If Needed)

**Goal:** Add structured metadata if required

**Source:** `audio-encryption-moq-codex/apps/chat-ui/src/audio/packets.ts:1`

**Changes:**
1. Review wrapEncryptedPacket/unwrapEncryptedPacket helpers
2. Add only if we need epoch/generation/counter metadata
3. Update encryption pipeline to use structured packets

**Validation:**
- Verify metadata is preserved through encryption
- Check backward compatibility
- **Pause for approval before final cleanup**

---

### Phase 8: Final Validation & Cleanup

**Goal:** Ensure everything works, document remaining issues

**Tasks:**
1. Run `just ci` - must pass
2. Run all Playwright tests - capture final metrics
3. Compare baseline (Phase 0) vs final frame drop rate
4. If drops persist, investigate:
   - Timer alignment in Rust (crates/marmot-chat/src/media_crypto.rs:14)
   - Nonce scheduling (crates/marmot-chat/src/controller/services.rs:1)
5. Update plans/spaghetti.md with any remaining tech debt
6. Update AUDIO_QUALITY_RESEARCH.md with what was implemented

**Success Criteria:**
- No frame skips in normal operation
- Audio quality matches innpub/hang
- All tests pass
- Documentation updated

---

## Operating Rules

- **Pause after each phase** for manual validation and approval
- **No mocks** - use real infrastructure (moq-relay, network, async)
- **`just ci` must pass** before moving to next phase
- **Document in plans/spaghetti.md** as you discover issues
- **Real implementations only** - no stubs or fake code
- Test each change with audio-integrity.spec.js + existing tests

## Key References

- AUDIO_QUALITY_RESEARCH.md - Technical analysis of issues
- plans/MOQ_MARMOT_AV_PLAN.md - Original phased plan
- MOQ_MARMOT_AV_SPEC.md - Protocol/spec details
