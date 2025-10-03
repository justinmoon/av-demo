import { createEffect, createSignal, createMemo, onCleanup } from 'solid-js';
import { createStore } from 'solid-js/store';
import { For, Show } from 'solid-js';
import { getPublicKey } from 'nostr-tools';
import type { ChatSession, ChatMessage, ChatState } from '../types';
import type { ChatHandle, ChatCallbacks, ErrorInfo } from '../chat/controller';
import { hexToBytes, normalizeHex } from '../utils';
import { startAudioCapture, float32ToInt16 } from '../audio/capture';
import { createAudioPlayback, int16ToFloat32 } from '../audio/playback';
import { createAudioMoq } from '../bridge/audio-moq';
import type { AudioCaptureHandle } from '../audio/capture';
import type { AudioPlaybackHandle } from '../audio/playback';
import type { AudioMoqHandle } from '../bridge/audio-moq';

export interface ChatViewProps {
  session: ChatSession;
  startChat: (session: ChatSession, callbacks: ChatCallbacks) => Promise<ChatHandle>;
}

export type { ChatSession };

export function ChatView(props: ChatViewProps) {
  const [status, setStatus] = createSignal('Initializing…');
  const [chatState, setChatState] = createStore<ChatState>({ messages: [], commits: 0, members: [] });
  const [ready, setReady] = createSignal(false);
  const [sending, setSending] = createSignal(false);
  const [invitePubkey, setInvitePubkey] = createSignal('');
  const [inviteAdmin, setInviteAdmin] = createSignal(false);
  const [inviteError, setInviteError] = createSignal('');
  const [inviteSuccess, setInviteSuccess] = createSignal('');
  const [currentError, setCurrentError] = createSignal<ErrorInfo | null>(null);
  const [audioEnabled, setAudioEnabled] = createSignal(false);
  const [audioStatus, setAudioStatus] = createSignal('');

  // Debug counters for testing encrypted audio transmission
  const [encryptedFramesSent, setEncryptedFramesSent] = createSignal(0);
  const [encryptedFramesReceived, setEncryptedFramesReceived] = createSignal(0);

  let controller: ChatHandle | null = null;
  let runId = 0;
  let messageInput: HTMLTextAreaElement | undefined;
  let audioCapture: AudioCaptureHandle | null = null;
  let audioMoq: AudioMoqHandle | null = null;
  let peerPlayback: Map<string, AudioPlaybackHandle> = new Map();

  const selfPubkey = createMemo(() => {
    try {
      return getPublicKey(hexToBytes(props.session.secretHex));
    } catch (err) {
      console.warn('Failed to derive self pubkey', err);
      return '';
    }
  });

  const isAdmin = createMemo(() => {
    if (props.session.role === 'initial') {
      return true;
    }
    const me = selfPubkey();
    if (!me) return false;
    return chatState.members.some((member) => member.pubkey === me && member.isAdmin);
  });

  const syncWindowState = () => {
    if (typeof window === 'undefined') {
      return;
    }
    const snapshot: ChatState = {
      messages: chatState.messages.map((message) => ({ ...message })),
      commits: chatState.commits,
      members: chatState.members.map((member) => ({ ...member })),
    };
    (window as any).chatState = snapshot;
    (window as any).chatReady = ready();
    (window as any).chatStatus = status();
    (window as any).chatError = currentError()?.message ?? '';
    (window as any).audioStats = {
      encryptedFramesSent: encryptedFramesSent(),
      encryptedFramesReceived: encryptedFramesReceived(),
    };
  };

  createEffect(syncWindowState);
  syncWindowState();

  const callbacks: ChatCallbacks = {
    setStatus: (text) => setStatus(text),
    pushMessage: (message) => {
      setChatState('messages', (messages) => [...messages, message]);
    },
    setCommits: (count) => setChatState('commits', count),
    setReady: (value) => {
      console.debug('[marmot-chat ui] ready', value);
      setReady(value);
    },
    setRoster: (members) => {
      setChatState('members', members.map((member) => ({ ...member })));
    },
    upsertMember: (member) => {
      setChatState('members', (current) => {
        const next = [...current];
        const index = next.findIndex((item) => item.pubkey === member.pubkey);
        if (index >= 0) {
          next[index] = { ...next[index], ...member };
        } else {
          next.push({ ...member });
        }
        return next;
      });
    },
    removeMember: (pubkey) => {
      setChatState('members', (current) => current.filter((member) => member.pubkey !== pubkey));
    },
    showError: (error) => {
      setCurrentError(error);
      if (!error.fatal) {
        // Auto-dismiss non-fatal errors after 5 seconds
        setTimeout(() => {
          if (currentError()?.message === error.message && !currentError()?.fatal) {
            setCurrentError(null);
          }
        }, 5000);
      }
    },
    clearError: () => setCurrentError(null),
  };

  const stopController = () => {
    if (controller) {
      try {
        controller.stop();
      } catch (err) {
        console.warn('Failed to stop controller', err);
      }
      controller = null;
    }
  };

  const stopAudio = () => {
    if (audioCapture) {
      try {
        audioCapture.stop();
      } catch (err) {
        console.warn('Failed to stop audio capture', err);
      }
      audioCapture = null;
    }
    if (audioMoq) {
      try {
        audioMoq.close();
      } catch (err) {
        console.warn('Failed to close audio MoQ', err);
      }
      audioMoq = null;
    }
    for (const [pubkey, playback] of peerPlayback.entries()) {
      try {
        playback.stop();
      } catch (err) {
        console.warn(`Failed to stop playback for ${pubkey}`, err);
      }
    }
    peerPlayback.clear();
    setAudioEnabled(false);
    setAudioStatus('');
  };

  createEffect(() => {
    const session = props.session;
    if (!session) return;
    const currentRun = ++runId;

    stopController();
    setStatus('Initializing…');
    setChatState({ messages: [], commits: 0, members: [] });
    setReady(false);
    setCurrentError(null);
    if (messageInput) {
      messageInput.value = '';
    }

    (async () => {
      try {
        const handle = await props.startChat(session, callbacks);
        if (currentRun === runId) {
          controller = handle;
        } else {
          handle.stop();
        }
      } catch (err) {
        console.error('Failed to start chat', err);
        setStatus('Chat failed to start');
      }
    })();
  });

  onCleanup(() => {
    stopController();
    stopAudio();
    runId += 1;
    if (typeof window !== 'undefined') {
      delete (window as any).chatState;
      delete (window as any).chatReady;
      delete (window as any).chatStatus;
    }
  });

  const handleSubmit = async (event: Event) => {
    event.preventDefault();
    const content = (messageInput?.value ?? '').trim();
    if (!content || !controller) {
      return;
    }
    setSending(true);
    try {
      await controller.sendMessage(content);
      if (messageInput) {
        messageInput.value = '';
      }
    } catch (err) {
      console.error('Failed to send message', err);
    } finally {
      setSending(false);
    }
  };

  const handleRotate = async () => {
    if (!controller) return;
    setSending(true);
    try {
      await controller.rotate();
    } catch (err) {
      console.error('Rotate failed', err);
    } finally {
      setSending(false);
    }
  };

  const handleInviteSubmit = async (event: Event) => {
    event.preventDefault();
    if (!controller) return;
    setInviteError('');
    setInviteSuccess('');
    try {
      const normalized = normalizeHex(invitePubkey(), 'Pubkey');
      controller.invite(normalized, inviteAdmin());
      setInvitePubkey('');
      setInviteAdmin(false);
      setInviteSuccess('Invite sent! Share the original invite link with the new participant.');
    } catch (err) {
      setInviteError((err as Error).message);
    }
  };

  const handleAudioToggle = async () => {
    if (audioEnabled()) {
      stopAudio();
      return;
    }

    try {
      setAudioStatus('Starting audio...');
      const myPubkey = selfPubkey();
      if (!myPubkey || !controller) {
        throw new Error('Cannot start audio: missing controller or pubkey');
      }

      // Get MoQ root from controller (MLS-derived)
      const moqRoot = await controller.groupRoot();
      const trackLabel = `audio-${myPubkey.slice(0, 8)}`;
      const epoch = await controller.currentEpoch();

      // Derive media base key for our own audio track
      const myBaseKey = await controller.deriveMediaBaseKey(myPubkey, trackLabel);

      // Track: base keys for peers (derived when we see them)
      const peerBaseKeys = new Map<string, string>();

      // Frame counter for outgoing frames
      let frameCounter = 0;

      // Track frame counters for each peer
      const peerFrameCounters = new Map<string, number>();

      // Helper: build AAD for a frame (takes track label as parameter)
      const buildAAD = (trackLabelForAAD: string, groupSeq: number, frameIdx: number, isKeyframe: boolean): Uint8Array => {
        const encoder = new TextEncoder();
        const version = new Uint8Array([1]);
        const groupRootBytes = encoder.encode(moqRoot);
        const trackLabelBytes = encoder.encode(trackLabelForAAD);
        const epochBytes = new Uint8Array(8);
        new DataView(epochBytes.buffer).setBigUint64(0, BigInt(epoch), false); // big-endian
        const groupSeqBytes = new Uint8Array(8);
        new DataView(groupSeqBytes.buffer).setBigUint64(0, BigInt(groupSeq), false);
        const frameIdxBytes = new Uint8Array(8);
        new DataView(frameIdxBytes.buffer).setBigUint64(0, BigInt(frameIdx), false);
        const keyframeBytes = new Uint8Array([isKeyframe ? 1 : 0]);

        const parts = [version, groupRootBytes, trackLabelBytes, epochBytes, groupSeqBytes, frameIdxBytes, keyframeBytes];
        const totalLen = parts.reduce((sum, p) => sum + p.length, 0);
        const aad = new Uint8Array(totalLen);
        let offset = 0;
        for (const part of parts) {
          aad.set(part, offset);
          offset += part.length;
        }
        return aad;
      };

      // Create audio MoQ bridge
      const moq = await createAudioMoq(
        {
          relay: props.session.relay,
          moqRoot,
          myPubkey,
          trackLabel,
        },
        {
          onReady: () => {
            console.debug('[audio] MoQ ready');
            setAudioStatus('Audio connected (encrypted)');
          },
          onPeerAudio: async (peerPubkey, data) => {
            // Derive peer's base key if not already done
            if (!peerBaseKeys.has(peerPubkey)) {
              const peerTrackLabel = `audio-${peerPubkey.slice(0, 8)}`;
              const peerKey = await controller!.deriveMediaBaseKey(peerPubkey, peerTrackLabel);
              peerBaseKeys.set(peerPubkey, peerKey);
            }

            // Extract frame counter from first 4 bytes (big-endian u32)
            if (data.length < 4) {
              console.error('[audio] Invalid frame: too small', data.length);
              return;
            }

            const view = new DataView(data.buffer, data.byteOffset, data.byteLength);
            const peerFrameCounter = view.getUint32(0, false); // big-endian
            const ciphertext = new Uint8Array(data.buffer, data.byteOffset + 4, data.byteLength - 4);

            // Build AAD with PEER's track label
            const peerTrackLabel = `audio-${peerPubkey.slice(0, 8)}`;
            const groupSeq = 0;
            const isKeyframe = peerFrameCounter === 0;
            const aad = buildAAD(peerTrackLabel, groupSeq, peerFrameCounter, isKeyframe);

            const peerKey = peerBaseKeys.get(peerPubkey)!;

            try {
              const plaintext = await controller!.decryptAudioFrame(peerKey, ciphertext, peerFrameCounter, aad);

              // Get or create playback for this peer
              if (!peerPlayback.has(peerPubkey)) {
                const playback = await createAudioPlayback({
                  sampleRate: 48000,
                  channelCount: 1,
                });
                peerPlayback.set(peerPubkey, playback);
              }
              const playback = peerPlayback.get(peerPubkey)!;

              // Convert decrypted Int16 to Float32 for playback
              const int16 = new Int16Array(plaintext.buffer, plaintext.byteOffset, plaintext.byteLength / 2);
              const float32 = int16ToFloat32(int16);
              playback.play(float32);

              // Track successful decryption
              setEncryptedFramesReceived((prev) => prev + 1);

              // Test hook: expose received audio for validation
              if ((window as any).audioTestData?.receivedFrames) {
                (window as any).audioTestData.receivedFrames.push(new Float32Array(float32));
              }

              // Track frame counter for debugging
              const currentCounter = peerFrameCounters.get(peerPubkey) || -1;

              // Debug: log every 50th received frame
              if (peerFrameCounter % 50 === 0) {
                console.debug('[audio] Received frame', peerFrameCounter, 'from', peerPubkey.slice(0, 8));
              }

              if (peerFrameCounter !== currentCounter + 1 && peerFrameCounter !== 0) {
                console.warn('[audio] Frame skip for peer', peerPubkey, 'expected', currentCounter + 1, 'got', peerFrameCounter);
              }
              peerFrameCounters.set(peerPubkey, peerFrameCounter);
            } catch (err) {
              console.error('[audio] Decrypt error for peer', peerPubkey, 'counter', peerFrameCounter, err);
            }
          },
          onError: (err) => {
            console.error('[audio] MoQ error', err);
            setAudioStatus(`Audio error: ${err}`);
          },
          onClosed: () => {
            console.debug('[audio] MoQ closed');
            setAudioStatus('');
          },
        }
      );

      audioMoq = moq;

      // Subscribe to all known peers
      for (const peer of chatState.members) {
        if (peer.pubkey !== myPubkey) {
          moq.subscribeToPeerAudio(peer.pubkey);
        }
      }

      // Start audio capture
      const capture = await startAudioCapture(
        {
          sampleRate: 48000,
          channelCount: 1,
          chunkSize: 1024, // ~21ms @ 48kHz (must be power of 2)
        },
        {
          onChunk: async (pcmData) => {
            // Test hook: expose sent audio for validation
            if ((window as any).audioTestData?.sentFrames) {
              (window as any).audioTestData.sentFrames.push(new Float32Array(pcmData));
            }

            // Convert Float32 to Int16
            const int16 = float32ToInt16(pcmData);
            const plaintext = new Uint8Array(int16.buffer);

            // Build AAD for this frame
            const groupSeq = 0; // MoQ group sequence (would increment for new groups)
            const isKeyframe = frameCounter === 0;
            const aad = buildAAD(trackLabel, groupSeq, frameCounter, isKeyframe);

            try {
              // Encrypt the frame
              const ciphertext = await controller!.encryptAudioFrame(myBaseKey, plaintext, frameCounter, aad);

              // Prepend frame counter (4 bytes big-endian u32) to encrypted payload
              const payload = new Uint8Array(4 + ciphertext.length);
              new DataView(payload.buffer).setUint32(0, frameCounter, false); // big-endian
              payload.set(ciphertext, 4);

              // Publish encrypted frame with metadata
              moq.publishAudio(payload);
              setEncryptedFramesSent((prev) => prev + 1);

              // Debug: log every 50th frame to track what's being sent
              if (frameCounter % 50 === 0) {
                console.debug('[audio] Published frame', frameCounter);
              }

              frameCounter++;
            } catch (err) {
              console.error('[audio] Encrypt error', err);
            }
          },
          onError: (err) => {
            console.error('[audio] Capture error', err);
            setAudioStatus(`Capture error: ${err.message}`);
            stopAudio();
          },
        }
      );

      audioCapture = capture;
      setAudioEnabled(true);
      setAudioStatus('Audio active (encrypted)');
    } catch (err) {
      console.error('[audio] Failed to start audio', err);
      setAudioStatus(`Failed: ${(err as Error).message}`);
      stopAudio();
    }
  };

  const formatRole = (role: string) => {
    return role === 'initial' ? 'Creator' : 'Invitee';
  };

  const getRecoveryMessage = (action?: string) => {
    switch (action) {
      case 'retry':
        return 'Please try again.';
      case 'refresh':
        return 'Please refresh the page to restart the session.';
      case 'check_connection':
        return 'Please check your network connection.';
      default:
        return '';
    }
  };

  const dismissError = () => setCurrentError(null);

  return (
    <main class="chat-app" id="chat-view-root">
      <header class="chat-app__header">
        <h1>Marmot Chat</h1>
        <div id="status" class="status">{status()}</div>
        <div class="info">
          <span id="role">Role: {formatRole(props.session.role)}</span>
          <span id="relay">Relay: {props.session.relay}</span>
          <span id="nostr">Nostr: {props.session.nostr}</span>
        </div>
      </header>

      <Show when={currentError()}>
        {(error) => (
          <div
            class={`error-banner error-banner--${error().fatal ? 'error' : 'warning'}`}
            role="alert"
            aria-live="assertive"
          >
            <div class="error-banner__content">
              <strong class="error-banner__title">
                {error().fatal ? 'Error' : 'Warning'}
              </strong>
              <p class="error-banner__message">{error().message}</p>
              <Show when={error().recoveryAction && getRecoveryMessage(error().recoveryAction)}>
                {(msg) => <p class="error-banner__recovery">{msg()}</p>}
              </Show>
            </div>
            <button
              class="error-banner__dismiss"
              onClick={dismissError}
              aria-label="Dismiss error"
            >
              ×
            </button>
          </div>
        )}
      </Show>

      <section class="chat-app__messages" id="messages" aria-live="polite">
        <For each={chatState.messages}>
          {(message) => (
            <div
              classList={{
                message: true,
                'message--self': message.local,
                'message--system': !!message.system,
              }}
            >
              <div class="message__meta">
                {message.system ? 'System' : message.local ? 'You' : shortenKey(message.author)} ·{' '}
                {new Date(message.createdAt * 1000).toLocaleTimeString()}
              </div>
              <div class="message__content">{message.content}</div>
            </div>
          )}
        </For>
      </section>

      <section class="chat-app__roster" id="members" aria-live="polite">
        <h2>Members</h2>
        <ul>
          <For each={chatState.members}>
            {(member) => (
              <li
                classList={{
                  member: true,
                  'member--admin': member.isAdmin,
                }}
              >
                <span class="member__pubkey">{shortenKey(member.pubkey)}</span>
                {member.isAdmin && <span class="member__role">admin</span>}
              </li>
            )}
          </For>
        </ul>
      </section>

      <Show when={isAdmin()}>
        <section class="chat-app__invite" id="invite">
          <h2>Add Participant</h2>
          <form onSubmit={handleInviteSubmit} autocomplete="off">
            <label for="invite-pubkey">Participant pubkey (hex)</label>
            <input
              id="invite-pubkey"
              data-testid="invite-pubkey"
              value={invitePubkey()}
              onInput={(event) => setInvitePubkey(event.currentTarget.value)}
              placeholder="64 hex characters"
            />
            <label class="invite-admin">
              <input
                type="checkbox"
                data-testid="invite-admin"
                checked={inviteAdmin()}
                onChange={(event) => setInviteAdmin(event.currentTarget.checked)}
              />
              Grant admin
            </label>
            <Show when={inviteError()}>{(err) => <div class="form-error">{err()}</div>}</Show>
            <Show when={inviteSuccess()}>{(msg) => <div class="form-success">{msg()}</div>}</Show>
            <button type="submit" data-testid="invite-submit" disabled={sending()}>
              Request invite
            </button>
          </form>
        </section>
      </Show>

      <footer class="chat-app__footer">
        <form id="composer" autocomplete="off" onSubmit={handleSubmit}>
          <label class="sr-only" for="message">
            Message
          </label>
          <textarea
            id="message"
            name="message"
            rows={2}
            placeholder="Type a message…"
            required
            ref={(el) => (messageInput = el)}
          />
          <button type="submit" id="send-message" disabled={sending() || !ready() || (currentError()?.fatal ?? false)}>
            Send
          </button>
          <button
            type="button"
            id="rotate"
            onClick={handleRotate}
            disabled={sending() || !ready() || (currentError()?.fatal ?? false)}
          >
            Rotate Epoch
          </button>
          <button
            type="button"
            id="audio-toggle"
            data-testid="audio-toggle"
            onClick={handleAudioToggle}
            disabled={!ready() || (currentError()?.fatal ?? false)}
            classList={{ active: audioEnabled() }}
          >
            {audioEnabled() ? '🎤 Stop Audio' : '🎤 Start Audio'}
          </button>
        </form>
        <Show when={audioStatus()}>
          <div id="audio-status" class="audio-status">
            {audioStatus()}
          </div>
        </Show>
      </footer>
    </main>
  );
}

function shortenKey(key: string, length = 6) {
  if (!key) return '';
  if (key.length <= length * 2 + 1) return key;
  return `${key.slice(0, length)}…${key.slice(-length)}`;
}

function formatRole(role: ChatSession['role']) {
  switch (role) {
    case 'initial':
      return 'Creator';
    case 'invitee':
      return 'Invitee';
    default:
      return role;
  }
}
