# Audio Quality Research: innpub vs hang vs our implementation

## Summary of Findings

After analyzing innpub and hang audio implementations, I've identified several key differences that likely contribute to their better audio quality. The main issues with our current implementation are:

1. **Wrong chunk size** - We use 1024 samples (~21ms) but should use 960 samples (20ms exactly)
2. **Using deprecated ScriptProcessorNode** instead of AudioWorklet
3. **No ring buffer** for playback - just scheduling buffers directly
4. **Playback timing issue** - No latency compensation
5. **Missing transferables** - Not using transferable objects for performance

## Detailed Comparison

### 1. Audio Capture

#### Our Current Implementation (capture.ts:40)
```typescript
const processor = audioContext.createScriptProcessor(config.chunkSize, config.channelCount, config.channelCount);
// chunkSize: 1024 (~21ms @ 48kHz)
```

**Problems:**
- Uses deprecated `ScriptProcessorNode` (runs on main thread)
- Chunk size is 1024 (21.33ms) instead of 960 (20ms)
- Main thread processing causes timing jitter
- Can drop frames under CPU load

#### innpub Implementation
```typescript
// capture.ts:42 - Uses AudioWorklet (runs in audio thread)
new AudioContext({ sampleRate: 48000 });
new AudioWorkletNode(context, "innpub-capture-processor");

// capture-worklet.js:44 - Uses transferable objects
this.port.postMessage(
  { type: "samples", channels, level: rms, sampleRate },
  channels.map(channel => channel.buffer)  // Transfer, don't copy!
);
```

**Benefits:**
- AudioWorklet runs in real-time audio thread (no main thread blocking)
- Uses transferables for zero-copy message passing
- More precise timing
- Calculates RMS level for monitoring

#### hang Implementation
```typescript
// capture-worklet.ts:14 - Even simpler, just timestamps
const timestamp = Time.Micro.fromSecond((this.#sampleCount / sampleRate) as Time.Second);
const msg: AudioFrame = { timestamp, channels };
this.port.postMessage(msg);
this.#sampleCount += channels[0].length;
```

**Benefits:**
- Tracks precise timestamps based on sample count
- No buffering in worklet, just pass through
- Let receiver handle timing

### 2. Chunk Size / Frame Rate

#### Our Settings (ChatView.tsx:416)
```typescript
chunkSize: 1024  // ~21ms @ 48kHz
```

**Problem**: 1024 samples ÷ 48000 Hz = **21.33ms per chunk**
- This doesn't align with standard frame rates
- Creates timing drift over time
- 1000ms ÷ 21.33ms = 46.875 chunks/sec (awkward)

#### Correct Settings (as you mentioned)
```
48000 Hz sample rate
960 samples per chunk
= 20ms per chunk EXACTLY
= 50 chunks per second EXACTLY
```

**Why 960 is critical:**
- 960 samples ÷ 48000 Hz = **20ms exactly**
- 1000ms ÷ 20ms = **50 frames/sec (clean integer)**
- Aligns with Opus codec frame sizes (10, 20, 40, 60ms)
- No accumulated timing errors

#### innpub doesn't specify chunk size
- Relies on browser's default (usually 128 samples @ 48kHz = 2.67ms)
- Very low latency, but more overhead
- Works because they buffer on receive side

### 3. Audio Playback & Buffering

#### Our Current Implementation (playback.ts:20-36)
```typescript
const play = (pcmData: Float32Array) => {
  const audioBuffer = audioContext.createBuffer(config.channelCount, pcmData.length, config.sampleRate);
  audioBuffer.copyToChannel(data, 0);
  const source = audioContext.createBufferSource();
  source.buffer = audioBuffer;
  source.connect(audioContext.destination);

  const now = audioContext.currentTime;
  const startTime = Math.max(now, nextStartTime);
  source.start(startTime);
  nextStartTime = startTime + audioBuffer.duration;
};
```

**Problems:**
- No buffering - plays frames immediately as they arrive
- No compensation for network jitter
- If frame arrives late, audio glitches
- `Math.max(now, nextStartTime)` can cause drift

#### innpub Implementation (playback.ts:48)
```typescript
const startAt = Math.max(remote.nextTime, context.currentTime + 0.05);
//                                                              ^^^^^ 50ms latency!
```

**Benefits:**
- Adds 50ms buffer/latency before playback
- Smooths out network jitter
- Gives time for frames to arrive
- Still feels instant to users

#### hang Implementation (ring-buffer.ts)
```typescript
class AudioRingBuffer {
  constructor(props: { rate: number; channels: number; latency: Time.Milli }) {
    const samples = Math.ceil(props.rate * Time.Second.fromMilli(props.latency));
    this.#buffer[i] = new Float32Array(samples);
  }

  write(timestamp: Time.Micro, data: Float32Array[]): void {
    // Writes at timestamp position, fills gaps with zeros
    // Handles out-of-order frames
    // Tracks overflow and underflow
  }

  read(output: Float32Array[]): number {
    // Reads from current position
    // Returns 0 if still refilling
    // Warns on underflow
  }
}
```

**Benefits:**
- Ring buffer absorbs timing jitter
- Handles out-of-order frames gracefully
- Fills gaps with silence instead of glitching
- Tracks underflow/overflow for debugging
- Uses timestamps for precise positioning

### 4. Frame Timing & Synchronization

#### Our Implementation
- No explicit frame numbering
- Relies on MoQ object ordering
- No timestamp tracking
- No jitter buffer

**Problem**: If MoQ drops a frame or delivers out of order, we have no way to detect or recover

#### hang Implementation
- Timestamps every frame based on sample count
- Ring buffer uses timestamps to position data
- Handles discontinuities by filling with zeros
- Warns on underflow

**Benefits:**
- Can detect and report frame drops
- Handles out-of-order delivery
- Smooth playback despite network issues

## Recommendations

### High Priority (Fix Frame Skipping)

1. **Change chunk size from 1024 to 960**
   ```typescript
   chunkSize: 960  // 20ms @ 48kHz, exactly 50 fps
   ```
   This alone may fix the frame skipping issue!

2. **Add playback latency buffer (50-100ms)**
   ```typescript
   const startTime = Math.max(now + 0.05, nextStartTime);
   ```

3. **Verify sample rate is actually 48000**
   - Browser may ignore our request
   - Check `audioContext.sampleRate` after creation
   - Log mismatches

### Medium Priority (Better Quality)

4. **Migrate to AudioWorklet** (from ScriptProcessorNode)
   - Use transferable objects
   - Better timing, no main thread blocking
   - Based on innpub's capture-worklet.js

5. **Add ring buffer for playback**
   - Based on hang's AudioRingBuffer
   - Handles jitter and out-of-order frames
   - Configurable latency (50-200ms)

6. **Add frame timestamps**
   - Track sample count like hang does
   - Include timestamp in encrypted payload
   - Use for ring buffer positioning

### Low Priority (Polish)

7. **Add audio level monitoring**
   - Calculate RMS like innpub does
   - Show visual feedback
   - Detect silence (mute detection)

8. **Better error handling**
   - Warn on underflow (like hang)
   - Detect and report discontinuities
   - Track frame drop statistics

## The 48kHz / 960 samples / 20ms Pattern

This is the **golden standard** for WebRTC and real-time audio:

- **Sample rate**: 48000 Hz (broadcast quality)
- **Frame size**: 960 samples
- **Frame duration**: 960 / 48000 = 0.02 sec = **20ms**
- **Frame rate**: 1000ms / 20ms = **50 fps**

**Why this works:**
- Opus codec native frame size (10, 20, 40, 60ms)
- No floating point errors (960 = 2^6 * 3 * 5)
- Clean integer math: 48000 / 960 = 50 exactly
- Used by WebRTC, Discord, Zoom, etc.

**Why 1024 doesn't work:**
- 1024 / 48000 = 0.02133... (repeating decimal)
- 48000 / 1024 = 46.875 (not integer)
- Accumulates timing drift
- Doesn't align with Opus frames

## Test This First

The simplest fix that might solve everything:

```typescript
// In ChatView.tsx:416
chunkSize: 960,  // Change from 1024
```

And:

```typescript
// In playback.ts:32
const startTime = Math.max(now + 0.05, nextStartTime);  // Add 50ms buffer
```

These two changes might completely fix the choppy audio!

## Playwright Test Suite Overview

We have several Playwright tests for validating audio quality and detecting frame drops:

### `tests/audio-integrity.spec.js`
**Purpose**: Validates audio transmission quality using deterministic test audio files

**What it tests**:
- Plays a real 3-second WAV file through the system
- Measures frame drop rate between sender and receiver
- Validates audio data integrity (RMS values, sample correctness)
- Compares sent vs received frame counts

**Useful for production testing**: ✅ **YES** - Set environment variables to test against production:
```bash
TEST_RELAY=https://moq.justinmoon.com/anon TEST_NOSTR=wss://damus.nostr.com npm test tests/audio-integrity.spec.js
```

**Key metrics**:
- Frame drop percentage (should be <20% over internet, 0% on localhost)
- Audio integrity validation (RMS values should match)

### `tests/step5-audio.spec.js`
**Purpose**: UI and basic audio functionality tests

**What it tests**:
- Audio toggle button appears
- Two participants can toggle audio independently
- MoQ audio tracks are created correctly
- Basic send/receive flow works

**Useful for production testing**: ⚠️ **LIMITED** - Tests basic functionality but doesn't measure quality or frame drops. Good for smoke testing but won't show network-related issues.

### `tests/step6-audio-e2e.spec.js`
**Purpose**: End-to-end encrypted audio transmission validation

**What it tests**:
- Deterministic audio file transmission (test-tone-3s.wav)
- Bit-for-bit decryption validation
- Frame counter integrity
- Encrypted audio frame structure

**Useful for production testing**: ✅ **YES** - Great for testing production relay:
```bash
# Run against production (modify relay path in test file first)
npm test tests/step6-audio-e2e.spec.js
```

**Key metrics**:
- Frame drop warnings in console
- Encrypted frames sent/received counts
- Audio data integrity after decryption

### `tests/debug-audio-frames.spec.js`
**Purpose**: Debug test for investigating frame skipping issues

**What it tests**:
- Currently skipped (test.skip)
- Was used during debugging to trace frame-by-frame behavior
- Logs detailed timing and sequence information

**Useful for production testing**: ❌ **NO** - Debug tool, currently disabled

### Best Test for Production Frame Drop Detection

**Recommended**: Use `tests/audio-integrity.spec.js` with production relay:

```bash
TEST_RELAY=https://moq.justinmoon.com/anon \
TEST_NOSTR=wss://damus.nostr.com \
npm test tests/audio-integrity.spec.js
```

This will show you:
1. Exact frame drop percentage
2. Whether VPN is causing issues (compare with/without VPN)
3. Audio quality degradation metrics
4. Network vs application issues (compare localhost vs production)

**Example output**:
```
Localhost:     0% drops (perfect)
Production:   15% drops (normal UDP packet loss)
With VPN:     66% drops (VPN interfering with UDP/QUIC)
```
