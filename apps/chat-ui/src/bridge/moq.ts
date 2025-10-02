import * as Moq from '@kixelated/moq';

const TRACK_NAME = 'wrappers';

export interface MoqConnectParams {
  relay: string;
  session: string;
  role: string;
  peerRole: string;
}

export interface MoqConnectCallbacks {
  onReady(): void;
  onFrame(data: Uint8Array): void;
  onError(message: unknown): void;
  onClosed(): void;
}

export interface MoqHandle {
  publish(data: Uint8Array): void;
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
      const roleValue = normalized.role;
      const peerRoleValue = normalized.peerRole;

      if (typeof relayValue !== 'string' || typeof sessionValue !== 'string' || typeof roleValue !== 'string' || typeof peerRoleValue !== 'string') {
        throw new Error('invalid MoQ connect params');
      }

      const relay = relayValue;
      const session = sessionValue;
      const role = roleValue;
      const peerRole = peerRoleValue;
      console.debug('[marmot-moq] connecting', { relay, session, role, peerRole });
      const connection = await Moq.Connection.connect(new URL(relay));
      let closed = false;

      const basePath = Moq.Path.from(session, TRACK_NAME);
      const publishPath = Moq.Path.join(basePath, Moq.Path.from(role));
      const subscribePath = Moq.Path.join(basePath, Moq.Path.from(peerRole));

      console.debug('[marmot-moq] publish path', publishPath.toString());
      console.debug('[marmot-moq] subscribe path', subscribePath.toString());

      const publisher = new Moq.Broadcast();
      connection.publish(publishPath, publisher);

      let currentTrack: Moq.Track | null = null;

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
            callbacks.onReady();
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

      const consumePeer = async () => {
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
            console.error('[marmot-moq] consume loop error', err);
            callbacks.onError(err);
            await new Promise((resolve) => setTimeout(resolve, 1000));
          }
        }
      };

      void consumePeer();

      const publish = (data: Uint8Array) => {
        if (currentTrack) {
          currentTrack.writeFrame(data);
        } else {
          console.warn('[marmot-moq] publish before track ready');
          callbacks.onError('Publish track not ready');
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
        close,
      };
    },
  };
}
