/**
 * AudioWorklet processor for capturing audio in real-time audio thread
 * Runs in a separate thread from main JS for better timing precision
 */
class MarmotCaptureProcessor extends AudioWorkletProcessor {
  process(inputs) {
    const input = inputs[0];
    if (!input || input.length === 0) {
      return true;
    }

    const channelCount = input.length;
    if (channelCount === 0) {
      return true;
    }

    const frameCount = input[0]?.length ?? 0;
    if (frameCount === 0) {
      return true;
    }

    // Copy channels and calculate RMS level
    const channels = new Array(channelCount);
    let total = 0;
    let sampleCount = 0;

    for (let channelIndex = 0; channelIndex < channelCount; channelIndex += 1) {
      const channel = input[channelIndex];
      const copy = new Float32Array(frameCount);
      copy.set(channel);
      channels[channelIndex] = copy;

      // Calculate RMS for level monitoring
      for (let frame = 0; frame < frameCount; frame += 1) {
        const sample = channel[frame];
        total += sample * sample;
        sampleCount += 1;
      }
    }

    const rms = sampleCount > 0 ? Math.sqrt(total / sampleCount) : 0;

    // Send samples to main thread using transferable objects (zero-copy)
    this.port.postMessage(
      {
        type: "samples",
        channels,
        level: rms,
        sampleRate,
      },
      channels.map(channel => channel.buffer),
    );

    return true;
  }
}

registerProcessor("marmot-capture-processor", MarmotCaptureProcessor);

export {};
