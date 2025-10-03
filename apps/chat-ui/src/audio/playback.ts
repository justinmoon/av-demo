/**
 * Simple audio playback using Web Audio API
 * Plays back PCM audio chunks with basic buffering
 */

export interface AudioPlaybackConfig {
  sampleRate: number;
  channelCount: number;
}

export interface AudioPlaybackHandle {
  play(pcmData: Float32Array): void;
  stop(): void;
}

export async function createAudioPlayback(config: AudioPlaybackConfig): Promise<AudioPlaybackHandle> {
  const audioContext = new AudioContext({ sampleRate: config.sampleRate });
  let nextStartTime = 0;

  const play = (pcmData: Float32Array) => {
    const audioBuffer = audioContext.createBuffer(config.channelCount, pcmData.length, config.sampleRate);
    // Create a new Float32Array with standard ArrayBuffer to satisfy TypeScript
    const data = new Float32Array(pcmData);
    audioBuffer.copyToChannel(data, 0);

    const source = audioContext.createBufferSource();
    source.buffer = audioBuffer;
    source.connect(audioContext.destination);

    // Schedule playback
    const now = audioContext.currentTime;
    const startTime = Math.max(now, nextStartTime);
    source.start(startTime);

    // Update next start time
    nextStartTime = startTime + audioBuffer.duration;
  };

  const stop = () => {
    audioContext.close();
  };

  return { play, stop };
}

/**
 * Convert Int16Array PCM to Float32Array for playback
 */
export function int16ToFloat32(int16: Int16Array): Float32Array {
  const float32 = new Float32Array(int16.length);
  for (let i = 0; i < int16.length; i++) {
    float32[i] = int16[i] / (int16[i] < 0 ? 0x8000 : 0x7fff);
  }
  return float32;
}
