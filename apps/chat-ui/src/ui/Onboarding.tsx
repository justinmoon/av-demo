import { createSignal, Show, Match, Switch, createEffect } from 'solid-js';
import { getPublicKey } from 'nostr-tools';
import {
  normalizeSecret,
  normalizeHex,
  randomHex,
  shortenKey,
  parseInvite,
  hexToBytes,
} from '../utils';
import type { ChatSession } from '../types';

export interface OnboardingResult {
  session: ChatSession;
}

interface Defaults {
  relay?: string;
  nostr?: string;
}

interface OnboardingProps {
  defaults: Defaults;
  onComplete(result: OnboardingResult): void;
}

type Step = 'login' | 'mode' | 'create' | 'invite' | 'join';

declare global {
  interface Nip07 {
    getPublicKey(): Promise<string>;
    getSecretKey?(): Promise<string>;
  }
}

export function Onboarding(props: OnboardingProps) {
  const [step, setStep] = createSignal<Step>('login');
  const [pubkey, setPubkey] = createSignal<string>('');
  const [secretHex, setSecretHex] = createSignal<string>('');
  const [loginError, setLoginError] = createSignal<string>('');
  const [relayUrl, setRelayUrl] = createSignal(
    props.defaults.relay ?? 'http://127.0.0.1:54943/marmot'
  );
  const [nostrUrl, setNostrUrl] = createSignal(props.defaults.nostr ?? 'ws://127.0.0.1:8880/');
  const [peerPub, setPeerPub] = createSignal('');
  const [inviteLink, setInviteLink] = createSignal('');
  const [sessionId, setSessionId] = createSignal('');
  const [joinError, setJoinError] = createSignal('');
  const [createError, setCreateError] = createSignal('');
  const [joinCode, setJoinCode] = createSignal('');
  const [manualSecretValue, setManualSecretValue] = createSignal('');
  let manualSecretInputRef: HTMLInputElement | undefined;

  createEffect(() => {
    const params = new URLSearchParams(window.location.search);
    const inviteParam = params.get('invite');
    if (inviteParam) {
      try {
        const decoded = decodeURIComponent(inviteParam);
        setJoinCode(decoded);
      } catch {
        setJoinCode(inviteParam);
      }
    }
  });

  const finalizeLogin = (secret: string, publicKey: string) => {
    setSecretHex(secret);
    setPubkey(publicKey);
    setLoginError('');
    setStep('mode');
  };

  const handleWalletConnect = async () => {
    setLoginError('');
    try {
      const nostr = (window as any).nostr as Nip07 | undefined;
      if (!nostr) {
        throw new Error('No NIP-07 extension found. Install Alby, nos2x, or another Nostr signer.');
      }
      const pub = await nostr.getPublicKey();
      // Generate ephemeral MLS secret locally (separate from Nostr identity)
      const mlsSecret = randomHex();
      finalizeLogin(mlsSecret, pub);
    } catch (err) {
      setLoginError((err as Error).message);
    }
  };

  const handleDevSecret = () => {
    const secret = randomHex();
    const pub = getPublicKey(hexToBytes(secret));
    finalizeLogin(secret, pub);
  };

  const handleManualSecret = (secret: string) => {
    try {
      const normalized = normalizeSecret(secret);
      const pub = getPublicKey(hexToBytes(normalized));
      finalizeLogin(normalized, pub);
    } catch (err) {
      setLoginError((err as Error).message);
    }
  };

  const handleCreateSubmit = (event: Event) => {
    event.preventDefault();
    setCreateError('');
    try {
      const peer = normalizeHex(peerPub(), 'Peer pubkey');
      setPeerPub(peer);
      const session = crypto.randomUUID().replace(/-/g, '');
      setSessionId(session);
      const invitePayload = { session, relay: relayUrl(), nostr: nostrUrl() };
      const encoded = encodeURIComponent(JSON.stringify(invitePayload));
      const link = `${window.location.origin}${window.location.pathname}?invite=${encoded}`;
      setInviteLink(link);
      setStep('invite');
    } catch (err) {
      setCreateError((err as Error).message);
    }
  };

  const handleJoinSubmit = (event: Event) => {
    event.preventDefault();
    setJoinError('');
    const parsed = parseInvite(joinCode());
    if (!parsed) {
      setJoinError('Invite link not valid');
      return;
    }
    setRelayUrl(parsed.relay);
    setNostrUrl(parsed.nostr);
    setSessionId(parsed.session);
    const session: ChatSession = {
      role: 'invitee',
      relay: parsed.relay,
      nostr: parsed.nostr,
      sessionId: parsed.session,
      secretHex: secretHex(),
      adminPubkeys: [],
      peerPubkeys: [],
    };
    props.onComplete({ session });
  };

  const enterChat = () => {
    const session: ChatSession = {
      role: 'initial',
      relay: relayUrl(),
      nostr: nostrUrl(),
      sessionId: sessionId(),
      secretHex: secretHex(),
      adminPubkeys: [pubkey()],
      peerPubkeys: peerPub() ? [peerPub()] : [],
    };
    props.onComplete({ session });
  };

  return (
    <section class="onboarding">
      <header class="onboarding__header">
        <h1>Marmot Chat</h1>
        <p class="tagline">Secure MLS chat over Nostr + MoQ</p>
      </header>

      <Switch fallback={null}>
        <Match when={step() === 'login'}>
          <section class="card">
            <h2>Step 1 · Connect Nostr Identity</h2>
            <p>Connect a NIP-07 browser extension (Alby, nos2x, etc.) or generate a temporary key.</p>
            <div class="actions">
              <button type="button" onClick={handleWalletConnect} data-testid="connect-nip07">
                Connect Extension
              </button>
              <button type="button" class="ghost" onClick={handleDevSecret} data-testid="use-dev-secret">
                Generate temp key
              </button>
            </div>
            <details class="manual-secret" open>
              <summary>Use existing secret</summary>
              <div class="manual-secret__content">
                <label for="manual-secret">Paste secret hex</label>
                <div class="manual-secret__row">
                  <input
                    id="manual-secret"
                    ref={manualSecretInputRef}
                    value={manualSecretValue()}
                    onInput={(event) => setManualSecretValue(event.currentTarget.value)}
                    placeholder="ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
                    autocomplete="off"
                    spellcheck={false}
                    data-testid="manual-secret-input"
                  />
                  <button
                    type="button"
                    data-testid="manual-secret-continue"
                    onClick={() => handleManualSecret(manualSecretValue().trim())}
                  >
                    Continue
                  </button>
                </div>
              </div>
            </details>
            <Show when={loginError()}>
              {(err) => <div class="login-status error">{err()}</div>}
            </Show>
          </section>
        </Match>

        <Match when={step() === 'mode'}>
          <section class="card">
            <h2>Step 2 · Start chatting</h2>
            <div class="pubkey-display">
              <label>Your pubkey</label>
              <div class="pubkey-row">
                <input readonly value={pubkey()} data-testid="mode-pubkey" />
                <button
                  type="button"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(pubkey());
                    } catch (err) {
                      console.warn('Failed to copy pubkey', err);
                    }
                  }}
                >
                  Copy
                </button>
              </div>
            </div>
            <div class="actions">
              <button type="button" data-testid="start-create" onClick={() => setStep('create')}>
                Create new chat
              </button>
              <button type="button" class="ghost" data-testid="start-join" onClick={() => setStep('join')}>
                Join invite
              </button>
            </div>
          </section>
        </Match>

        <Match when={step() === 'create'}>
          <section class="card">
            <h2>Create chat</h2>
            <div class="pubkey-display">
              <label>Your pubkey (share this with your peer)</label>
              <div class="pubkey-row">
                <input readonly value={pubkey()} data-testid="own-pubkey" />
                <button
                  type="button"
                  onClick={async () => {
                    try {
                      await navigator.clipboard.writeText(pubkey());
                    } catch (err) {
                      console.warn('Failed to copy pubkey', err);
                    }
                  }}
                >
                  Copy
                </button>
              </div>
            </div>
            <form onSubmit={handleCreateSubmit}>
              <label for="create-peer">Peer pubkey (paste their pubkey here)</label>
              <input
                id="create-peer"
                data-testid="create-peer"
                value={peerPub()}
                onInput={(event) => setPeerPub(event.currentTarget.value.trim())}
                placeholder="Paste your peer's pubkey"
              />

              <label for="create-relay">MoQ relay URL</label>
              <input
                id="create-relay"
                data-testid="create-relay"
                value={relayUrl()}
                onInput={(event) => setRelayUrl(event.currentTarget.value)}
              />

              <label for="create-nostr">Nostr relay URL</label>
              <input
                id="create-nostr"
                data-testid="create-nostr"
                value={nostrUrl()}
                onInput={(event) => setNostrUrl(event.currentTarget.value)}
              />

              <p class="hint">An invite link will be generated after you create the group.</p>
              <Show when={createError()}>{(err) => <div class="form-error">{err()}</div>}</Show>
              <div class="actions">
                <button type="submit" data-testid="create-submit">
                  Create chat
                </button>
                <button type="button" class="ghost" id="create-cancel" onClick={() => setStep('mode')}>
                  Back
                </button>
              </div>
            </form>
          </section>
        </Match>

        <Match when={step() === 'invite'}>
          <section class="card">
            <h2>Invite link</h2>
            <p>Share this link with the invited participant.</p>
            <textarea id="invite-link" readonly value={inviteLink()} data-testid="invite-link" />
            <div class="actions">
              <button
                type="button"
                id="copy-invite"
                data-testid="copy-invite"
                onClick={async () => {
                  try {
                    await navigator.clipboard.writeText(inviteLink());
                  } catch (err) {
                    console.warn('Failed to copy invite link', err);
                  }
                }}
              >
                Copy link
              </button>
              <button type="button" id="enter-chat" data-testid="enter-chat" onClick={enterChat}>
                Enter chat
              </button>
            </div>
          </section>
        </Match>

        <Match when={step() === 'join'}>
          <section class="card">
            <h2>Join chat</h2>
            <form onSubmit={handleJoinSubmit}>
              <label for="join-code">Paste invite link or code</label>
              <textarea
                id="join-code"
                value={joinCode()}
                onInput={(event) => setJoinCode(event.currentTarget.value)}
                data-testid="join-code"
              />

              <label for="join-relay">MoQ relay URL</label>
              <input
                id="join-relay"
                value={relayUrl()}
                onInput={(event) => setRelayUrl(event.currentTarget.value)}
                data-testid="join-relay"
              />

              <label for="join-nostr">Nostr relay URL</label>
              <input
                id="join-nostr"
                value={nostrUrl()}
                onInput={(event) => setNostrUrl(event.currentTarget.value)}
                data-testid="join-nostr"
              />

              <Show when={joinError()}>{(err) => <div class="form-error">{err()}</div>}</Show>
              <div class="actions">
                <button type="submit" data-testid="join-submit">
                  Join chat
                </button>
                <button type="button" class="ghost" id="join-cancel" onClick={() => setStep('mode')}>
                  Back
                </button>
              </div>
            </form>
          </section>
        </Match>
      </Switch>
    </section>
  );
}
