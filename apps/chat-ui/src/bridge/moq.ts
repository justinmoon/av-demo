import * as Moq from '@kixelated/moq';

const TRACK_NAME = 'wrappers';

export interface MoqConnectParams {
  relay: string;
  session: string;
  pubkey: string;
  peerPubkeys: string[];
}

export interface MoqConnectCallbacks {
  onReady(): void;
  onFrame(data: Uint8Array): void;
  onError(message: unknown): void;
  onClosed(): void;
}

export interface MoqHandle {
  publish(data: Uint8Array): void;
  subscribeToPeer(peerPubkey: string): void;
  close(): void;
}
export async function createMoqBridge() {
  if ((globalThis as any).__MARMOT_MOQ__) {
    return;
  }
  (globalThis as any).__MARMOT_MOQ__ = {
    connect: async (params: any, callbacks: MoqConnectCallbacks): Promise<MoqHandle> => {
      console.debug('[marmot-moq] raw params', params);
      const normalized: Record<string, unknown> = params instanceof Map ? Object.fromEntries(params as Map<string, unknown>) : (params as Record<string, unknown>);
      const relayValue = normalized.relay;
      const sessionValue = normalized.session;
      const pubkeyValue = normalized.pubkey;
      const peerPubkeysValue = normalized.peerPubkeys;

      if (typeof relayValue !== 'string' || typeof sessionValue !== 'string' || typeof pubkeyValue !== 'string' || !Array.isArray(peerPubkeysValue)) {
        throw new Error('invalid MoQ connect params');
      }

      const relay = relayValue;
      const session = sessionValue;
      const pubkey = pubkeyValue;
      const peerPubkeys = peerPubkeysValue as string[];
      console.debug('[marmot-moq] connecting', { relay, session, pubkey, peerPubkeys });
      const connection = await Moq.Connection.connect(new URL(relay));
      let closed = false;

      const basePath = Moq.Path.from(session, TRACK_NAME);
      const publishPath = Moq.Path.join(basePath, Moq.Path.from(pubkey));

      console.debug('[marmot-moq] publish path', publishPath.toString());
      console.debug('[marmot-moq] subscribing to', peerPubkeys.length, 'peer tracks');

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

      const acquireTrack = async () => {
        try {
          for (;;) {
            const request = await publisher.requested();
            if (!request) break;
            const track = request.track as Moq.Track;
            console.debug('[marmot-moq] track requested', track.name);
            if (track.name !== TRACK_NAME) {
              track.close();
              continue;
            }
            currentTrack = track;
            console.debug('[marmot-moq] publish track ready', { track: track.name });
            callOnReady();
            flushPendingPublish();
            try {
              await track.closed;
            } catch (err) {
              console.error('[marmot-moq] publish track closed with error', err);
              callbacks.onError(err);
            } finally {
              if (currentTrack === track) {
                currentTrack = null;
              }
            }
          }
        } catch (err) {
          console.error('[marmot-moq] publish loop error', err);
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

      const consumePeerTrack = async (peerPubkey: string) => {
        const subscribePath = Moq.Path.join(basePath, Moq.Path.from(peerPubkey));
        console.debug('[marmot-moq] subscribing to peer', peerPubkey, 'path:', subscribePath.toString());

        while (!closed) {
          try {
            const broadcast = connection.consume(subscribePath);
            const track = broadcast.subscribe(TRACK_NAME, 0);
            for (;;) {
              const frame = await track.readFrame();
              if (!frame) break;
              callbacks.onFrame(frame);
            }
          } catch (err) {
            if (isTransient(err)) {
              console.warn('[marmot-moq] transient consume error for peer', peerPubkey, 'retrying', err);
            } else {
              console.error('[marmot-moq] consume loop error for peer', peerPubkey, err);
              callbacks.onError(err);
            }
            await new Promise((resolve) => setTimeout(resolve, 1000));
          }
        }
      };

      // Subscribe to all peer tracks
      for (const peerPubkey of peerPubkeys) {
        if (peerPubkey !== pubkey) {
          void consumePeerTrack(peerPubkey);
        }
      }

      // Mark as ready after a short delay to allow publish setup to complete
      // This ensures we're ready even if no one subscribes to our track yet
      setTimeout(() => callOnReady(), 100);

      const pendingPublish: Uint8Array[] = [];

      const publish = (data: Uint8Array) => {
        if (currentTrack) {
          currentTrack.writeFrame(data);
        } else {
          console.warn('[marmot-moq] publish before track ready, queuing');
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

      const subscribeToPeer = (peerPubkey: string) => {
        console.debug('[marmot-moq] subscribeToPeer', peerPubkey);
        if (peerPubkey !== pubkey) {
          void consumePeerTrack(peerPubkey);
        }
      };

      const close = () => {
        if (closed) return;
        closed = true;
        try {
          connection.close();
        } catch (err) {
          console.error('[marmot-moq] close error', err);
          callbacks.onError(err);
        }
        callbacks.onClosed();
      };

      return {
        publish,
        subscribeToPeer,
        close,
      };
    },
  };
}
