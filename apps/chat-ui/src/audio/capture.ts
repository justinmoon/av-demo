/**
 * Simple audio capture using Web Audio API
 * Captures mic input at 48kHz mono and provides PCM audio chunks
 */

export interface AudioCaptureConfig {
  sampleRate: number; // e.g., 48000
  channelCount: number; // 1 for mono, 2 for stereo
  chunkSize: number; // frames per chunk (e.g., 960 for 20ms @ 48kHz)
}

export interface AudioCaptureCallbacks {
  onChunk(pcmData: Float32Array): void;
  onError(error: Error): void;
}

export interface AudioCaptureHandle {
  stop(): void;
}

export async function startAudioCapture(
  config: AudioCaptureConfig,
  callbacks: AudioCaptureCallbacks
): Promise<AudioCaptureHandle> {
  // Request microphone access
  const stream = await navigator.mediaDevices.getUserMedia({
    audio: {
      sampleRate: config.sampleRate,
      channelCount: config.channelCount,
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
    },
  });

  const audioContext = new AudioContext({ sampleRate: config.sampleRate });
  const source = audioContext.createMediaStreamSource(stream);

  // Use ScriptProcessorNode for now (AudioWorklet would be better but more complex)
  const processor = audioContext.createScriptProcessor(config.chunkSize, config.channelCount, config.channelCount);

  processor.onaudioprocess = (event) => {
    try {
      const inputBuffer = event.inputBuffer;
      const channelData = inputBuffer.getChannelData(0); // Get first channel

      // Copy the data (Float32Array)
      const chunk = new Float32Array(channelData);
      callbacks.onChunk(chunk);
    } catch (error) {
      callbacks.onError(error instanceof Error ? error : new Error(String(error)));
    }
  };

  source.connect(processor);
  processor.connect(audioContext.destination);

  const stop = () => {
    processor.disconnect();
    source.disconnect();
    audioContext.close();
    stream.getTracks().forEach((track) => track.stop());
  };

  return { stop };
}

/**
 * Convert Float32Array PCM to Int16Array for transmission
 * (Most codecs expect 16-bit PCM)
 */
export function float32ToInt16(float32: Float32Array): Int16Array {
  const int16 = new Int16Array(float32.length);
  for (let i = 0; i < float32.length; i++) {
    const s = Math.max(-1, Math.min(1, float32[i]));
    int16[i] = s < 0 ? s * 0x8000 : s * 0x7fff;
  }
  return int16;
}
