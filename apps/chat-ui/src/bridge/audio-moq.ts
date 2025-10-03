import * as Moq from '@kixelated/moq';

export interface AudioMoqConfig {
  relay: string;
  moqRoot: string; // e.g., "marmot/abc123"
  myPubkey: string;
  trackLabel: string; // e.g., "alice-audio-001"
}

export interface AudioMoqCallbacks {
  onReady(): void;
  onPeerAudio(peerPubkey: string, data: Uint8Array): void;
  onError(message: unknown): void;
  onClosed(): void;
}

export interface AudioMoqHandle {
  publishAudio(data: Uint8Array): void;
  subscribeToPeerAudio(peerPubkey: string): void;
  close(): void;
}

/**
 * Create MoQ bridge for multi-track audio streaming
 *
 * Path structure:
 * - Publish: {moqRoot}/audio/{myPubkey}/{trackLabel}
 * - Subscribe: {moqRoot}/audio/{peerPubkey}/{trackLabel}
 */
export async function createAudioMoq(
  config: AudioMoqConfig,
  callbacks: AudioMoqCallbacks
): Promise<AudioMoqHandle> {
  console.debug('[audio-moq] connecting', config);

  const connection = await Moq.Connection.connect(new URL(config.relay));
  let closed = false;

  // Build publish path: moqRoot/audio/myPubkey
  const basePath = Moq.Path.from(config.moqRoot);
  const audioPath = Moq.Path.join(basePath, Moq.Path.from('audio'));
  const myAudioPath = Moq.Path.join(audioPath, Moq.Path.from(config.myPubkey));
  const publishPath = Moq.Path.join(myAudioPath, Moq.Path.from(config.trackLabel));

  console.debug('[audio-moq] publish path:', publishPath.toString());

  const publisher = new Moq.Broadcast();
  connection.publish(publishPath, publisher);

  let currentTrack: Moq.Track | null = null;
  let readyCalled = false;

  const callOnReady = () => {
    if (!readyCalled) {
      readyCalled = true;
      callbacks.onReady();
    }
  };

  // Handle track requests for our audio
  const acquireTrack = async () => {
    try {
      for (;;) {
        const request = await publisher.requested();
        if (!request) break;

        const track = request.track as Moq.Track;
        console.debug('[audio-moq] track requested:', track.name);

        if (track.name !== config.trackLabel) {
          console.warn('[audio-moq] unexpected track name:', track.name);
          track.close();
          continue;
        }

        currentTrack = track;
        console.debug('[audio-moq] publish track ready');
        callOnReady();
        flushPendingPublish();

        try {
          await track.closed;
        } catch (err) {
          console.error('[audio-moq] publish track closed with error', err);
          callbacks.onError(err);
        } finally {
          if (currentTrack === track) {
            currentTrack = null;
          }
        }
      }
    } catch (err) {
      console.error('[audio-moq] publish loop error', err);
      callbacks.onError(err);
    }
  };

  void acquireTrack();

  const isTransient = (error: unknown) => {
    const message =
      typeof error === 'string'
        ? error
        : error instanceof Error
        ? error.message
        : String(error ?? '');
    return /reset_stream/i.test(message) || /not found/i.test(message);
  };

  // Subscribe to a peer's audio track
  const subscribeToPeerAudio = (peerPubkey: string) => {
    if (peerPubkey === config.myPubkey) {
      console.debug('[audio-moq] skipping self-subscription');
      return;
    }

    const consumePeerAudio = async () => {
      const peerAudioPath = Moq.Path.join(audioPath, Moq.Path.from(peerPubkey));
      const subscribePath = Moq.Path.join(peerAudioPath, Moq.Path.from(config.trackLabel));
      console.debug('[audio-moq] subscribing to peer audio:', peerPubkey, 'path:', subscribePath.toString());

      while (!closed) {
        try {
          const broadcast = connection.consume(subscribePath);
          const track = broadcast.subscribe(config.trackLabel, 0);

          for (;;) {
            const frame = await track.readFrame();
            if (!frame) break;
            callbacks.onPeerAudio(peerPubkey, frame);
          }
        } catch (err) {
          if (isTransient(err)) {
            console.warn('[audio-moq] transient error for peer', peerPubkey, 'retrying', err);
          } else {
            console.error('[audio-moq] consume error for peer', peerPubkey, err);
            callbacks.onError(err);
          }
          await new Promise((resolve) => setTimeout(resolve, 1000));
        }
      }
    };

    void consumePeerAudio();
  };

  // Ready after delay to allow setup
  setTimeout(() => callOnReady(), 100);

  const pendingPublish: Uint8Array[] = [];

  const publishAudio = (data: Uint8Array) => {
    if (currentTrack) {
      currentTrack.writeFrame(data);
    } else {
      console.warn('[audio-moq] publish before track ready, queuing');
      pendingPublish.push(data);
    }
  };

  const flushPendingPublish = () => {
    while (pendingPublish.length > 0 && currentTrack) {
      const data = pendingPublish.shift();
      if (data) {
        currentTrack.writeFrame(data);
      }
    }
  };

  const close = () => {
    if (closed) return;
    closed = true;
    try {
      connection.close();
    } catch (err) {
      console.error('[audio-moq] close error', err);
      callbacks.onError(err);
    }
    callbacks.onClosed();
  };

  return {
    publishAudio,
    subscribeToPeerAudio,
    close,
  };
}
