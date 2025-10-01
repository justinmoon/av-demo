import initWasm, {
  accept_welcome,
  create_group,
  create_identity,
  create_message,
  create_key_package,
  export_key_package_bundle,
  import_key_package_bundle,
  ingest_wrapper,
  merge_pending_commit,
  public_key,
  self_update,
  list_groups,
} from '../../tests/pkg/marmot_chat.js';
import * as Moq from '@kixelated/moq';
import { Relay, finalizeEvent } from 'nostr-tools';
import { hexToBytes } from '@noble/hashes/utils';

const MoqAny: any = Moq;

const ALICE_SECRET = '4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae';
const BOB_SECRET = '6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd';
const TRACK_NAME = 'wrappers';
const HANDSHAKE_RETRY_MS = 2000;
const HANDSHAKE_KIND = 44501;
const encoder = new TextEncoder();
const decoder = new TextDecoder();

type Role = 'alice' | 'bob';

interface HandshakeEnvelope {
  session: string;
  from: Role;
  created_at?: number;
}

type RequestKeyPackageHandshake = HandshakeEnvelope & { type: 'request-key-package' };
type RequestWelcomeHandshake = HandshakeEnvelope & { type: 'request-welcome' };
type KeyPackageHandshake = HandshakeEnvelope & {
  type: 'key-package';
  event: string;
  bundle: string;
  pubkey: string;
};
type WelcomeHandshake = HandshakeEnvelope & {
  type: 'welcome';
  welcome: string;
  groupIdHex: string;
};

type IncomingHandshake =
  | RequestKeyPackageHandshake
  | RequestWelcomeHandshake
  | KeyPackageHandshake
  | WelcomeHandshake;

type OutgoingHandshake =
  | { type: 'request-key-package' }
  | { type: 'request-welcome' }
  | { type: 'key-package'; event: string; bundle: string; pubkey: string }
  | { type: 'welcome'; welcome: string; groupIdHex: string };

type HandshakeType = IncomingHandshake['type'];

interface DecryptedMessage {
  content: string;
  author: string;
  created_at: number;
}

interface ProcessedWrapper {
  kind: string;
  message?: DecryptedMessage;
  commit?: { event: string };
  proposal?: unknown;
}

interface ChatMessage {
  content: string;
  author: string;
  createdAt: number;
  local: boolean;
}

interface ChatState {
  messages: ChatMessage[];
  commits: number;
}

declare global {
  interface Window {
    chatReady?: boolean;
    chatState?: ChatState;
    sendTestMessage?: (content: string) => Promise<void>;
    triggerCommit?: () => Promise<void>;
  }
}

function byId<T extends HTMLElement>(id: string): T {
  const el = document.getElementById(id);
  if (!el) throw new Error(`Missing element #${id}`);
  return el as T;
}

function createDeferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason?: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

function delay(ms: number) {
  return new Promise((res) => setTimeout(res, ms));
}

class NostrHandshake {
  private relay: Relay | null = null;
  private subscription: any = null;
  private readonly handlers: Map<HandshakeType, Set<(msg: IncomingHandshake) => void>> = new Map();
  private readonly secretHex: string;
  private readonly secretBytes: Uint8Array;
  private readonly seenIds: Set<string> = new Set();

  constructor(
    private readonly url: string,
    private readonly session: string,
    private readonly role: Role,
    secretHex: string
  ) {
    this.secretHex = secretHex;
    this.secretBytes = hexToBytes(secretHex);
  }

  async connect(): Promise<void> {
    this.relay = await Relay.connect(this.url);
    this.subscription = this.relay.subscribe(
      [
        {
          kinds: [HANDSHAKE_KIND],
          '#t': [this.session],
          limit: 50,
        },
      ],
      {
        onevent: (event: any) => {
          if (!event || typeof event.content !== 'string') {
            return;
          }
          if (this.seenIds.has(event.id)) {
            return;
          }
          this.seenIds.add(event.id);
          try {
            const payload = JSON.parse(event.content) as IncomingHandshake;
            if (!payload || payload.session !== this.session) {
              return;
            }
            const handlers = this.handlers.get(payload.type as HandshakeType);
            if (!handlers || handlers.size === 0) {
              return;
            }
            handlers.forEach((handler) => {
              try {
                handler(payload);
              } catch (err) {
                console.error('Handshake handler failed', err);
              }
            });
          } catch (err) {
            console.error('Failed to parse handshake payload', err, event.content);
          }
        },
      }
    );
  }

  on<T extends HandshakeType>(
    type: T,
    handler: (payload: Extract<IncomingHandshake, { type: T }>) => void
  ): () => void {
    const wrapped = (payload: IncomingHandshake) => {
      handler(payload as Extract<IncomingHandshake, { type: T }>);
    };
    const set = this.handlers.get(type) ?? new Set();
    set.add(wrapped);
    this.handlers.set(type, set);
    return () => {
      set.delete(wrapped);
    };
  }

  async send(message: OutgoingHandshake): Promise<void> {
    if (!this.relay) {
      throw new Error('Handshake relay not connected');
    }
    const payload = {
      ...message,
      session: this.session,
      from: this.role,
      created_at: Math.floor(Date.now() / 1000),
    } satisfies IncomingHandshake;

    const eventTemplate = {
      kind: HANDSHAKE_KIND,
      created_at: payload.created_at,
      tags: [
        ['t', this.session],
        ['type', message.type],
        ['role', this.role],
      ],
      content: JSON.stringify(payload),
    };

    const signed = finalizeEvent(eventTemplate, this.secretBytes);
    await this.relay.publish(signed);
  }

  close() {
    try {
      this.subscription?.close?.();
    } catch (err) {
      console.warn('Failed to unsubscribe handshake', err);
    }
    try {
      this.relay?.close?.();
    } catch (err) {
      console.warn('Failed to close handshake relay', err);
    }
    this.handlers.clear();
    this.seenIds.clear();
  }
}

async function main() {
  const params = new URLSearchParams(window.location.search);
  const roleParam = (params.get('role') || '').toLowerCase();
  const relayParam = params.get('relay');
  const nostrParam = params.get('nostr');
  const session = params.get('session') || 'default';

  if (roleParam !== 'alice' && roleParam !== 'bob') {
    showHelp('Missing role parameter (alice or bob)');
    return;
  }
  if (!relayParam) {
    showHelp('Missing relay parameter');
    return;
  }
  if (!nostrParam) {
    showHelp('Missing nostr parameter');
    return;
  }

  const role = roleParam as Role;
  const peerRole: Role = role === 'alice' ? 'bob' : 'alice';
  let relayUrl: URL;
  let nostrUrl: URL;
  try {
    relayUrl = new URL(relayParam);
  } catch (err) {
    showHelp(`Invalid relay URL: ${(err as Error).message}`);
    return;
  }
  try {
    nostrUrl = new URL(nostrParam);
  } catch (err) {
    showHelp(`Invalid nostr URL: ${(err as Error).message}`);
    return;
  }

  await initWasm();

  const statusEl = byId<HTMLDivElement>('status');
  const messagesEl = byId<HTMLDivElement>('messages');
  const roleEl = byId<HTMLSpanElement>('role');
  const relayEl = byId<HTMLSpanElement>('relay');
  const nostrEl = byId<HTMLSpanElement>('nostr');
  const formEl = byId<HTMLFormElement>('composer');
  const messageInput = byId<HTMLTextAreaElement>('message');
  const rotateBtn = byId<HTMLButtonElement>('rotate');

  roleEl.textContent = `Role: ${role}`;
  relayEl.textContent = `Relay: ${relayUrl}`;
  nostrEl.textContent = `Nostr: ${nostrUrl}`;

  const state: {
    role: Role;
    peerRole: Role;
    relayUrl: URL;
    nostrUrl: URL;
    session: string;
    identityId: number;
    identityPub: string;
    peerPub?: string;
    groupIdHex?: string;
    broadcastBase?: string;
    publishPath?: string;
    subscribePath?: string;
    outgoingTrack?: any;
    outgoingDeferred: ReturnType<typeof createDeferred<any>>;
    connection?: any;
    chatState: ChatState;
  } = {
    role,
    peerRole,
    relayUrl,
    nostrUrl,
    session,
    identityId: 0,
    identityPub: '',
    outgoingDeferred: createDeferred<any>(),
    chatState: { messages: [], commits: 0 },
  };

  window.chatState = state.chatState;

  const keyPackageStore: {
    event?: string;
    bundle?: string;
    pubkey?: string;
  } = {};

  let handshakeComplete = false;
  let welcomeInterval: number | undefined;
  let keyPackageInterval: number | undefined;
  let handshake: NostrHandshake | undefined;
  const handshakeUnsubs: Array<() => void> = [];

  async function publishHandshake(message: OutgoingHandshake) {
    if (!handshake) return;
    try {
      await handshake.send(message);
    } catch (err) {
      console.error('Failed to publish handshake message', err);
    }
  }

  setStatus('Creating identity…');
  const secret = role === 'alice' ? ALICE_SECRET : BOB_SECRET;
  state.identityId = create_identity(secret);
  state.identityPub = public_key(state.identityId);

  setStatus('Connecting handshake relay…');
  try {
    handshake = new NostrHandshake(nostrUrl.toString(), session, role, secret);
    await handshake.connect();
  } catch (err) {
    console.error('Failed to connect handshake relay', err);
    setStatus(`Handshake relay error: ${(err as Error).message}`);
    return;
  }

  handshakeUnsubs.push(
    handshake.on('key-package', (payload) => {
      if (payload.from === role) {
        return;
      }
      if (role === 'alice' && payload.from === 'bob') {
        clearInterval(keyPackageInterval);
        keyPackageStore.event = payload.event;
        keyPackageStore.bundle = payload.bundle;
        keyPackageStore.pubkey = payload.pubkey;
        void handleAliceGroupCreation();
      }
    })
  );

  handshakeUnsubs.push(
    handshake.on('welcome', (payload) => {
      if (payload.from === role) {
        return;
      }
      if (role === 'bob' && payload.from === 'alice') {
        clearInterval(welcomeInterval);
        void handleBobAcceptWelcome(payload.welcome, payload.groupIdHex);
      }
    })
  );

  handshakeUnsubs.push(
    handshake.on('request-key-package', (payload) => {
      if (payload.from === role) {
        return;
      }
      if (role === 'bob') {
        void sendBobKeyPackage();
      }
    })
  );

  handshakeUnsubs.push(
    handshake.on('request-welcome', (payload) => {
      if (payload.from === role) {
        return;
      }
      if (role === 'alice' && state.groupIdHex && keyPackageStore.event) {
        void sendWelcome();
      }
    })
  );

  async function sendBobKeyPackage() {
    if (role !== 'bob') return;
    if (!keyPackageStore.event) {
      // Generate key package if not done yet
      const relays = [relayRelaysUrl(relayUrl)];
      const pkg = create_key_package(state.identityId, relays) as { event: string };
      keyPackageStore.event = pkg.event;
      const bundle = export_key_package_bundle(state.identityId, pkg.event) as {
        bundle: string;
      };
      keyPackageStore.bundle = bundle.bundle;
      keyPackageStore.pubkey = state.identityPub;
    }
    await publishHandshake({
      type: 'key-package',
      event: keyPackageStore.event!,
      bundle: keyPackageStore.bundle!,
      pubkey: keyPackageStore.pubkey!,
    });
  }

  async function handleAliceGroupCreation() {
    if (handshakeComplete || role !== 'alice') return;
    if (!keyPackageStore.event || !keyPackageStore.pubkey) return;
    setStatus('Creating group…');

    const config = {
      name: 'Marmot Chat',
      description: 'MoQ/MLS demo',
      relays: [relayRelaysUrl(relayUrl)],
      admins: [state.identityPub, keyPackageStore.pubkey],
    };
    const members = [keyPackageStore.event];
    const groupResp = create_group(state.identityId, config, members) as {
      group_id_hex: string;
      welcome: string[];
    };
    state.groupIdHex = groupResp.group_id_hex;
    await updateGroupMetadata();
    await sendWelcome(groupResp.welcome[0]);
    maybeFinishHandshake();
  }

  async function handleBobAcceptWelcome(welcome: string, groupIdHex: string) {
    if (handshakeComplete || role !== 'bob') return;
    if (!welcome) return;
    if (!keyPackageStore.bundle) {
      console.warn('Missing key package bundle; cannot accept welcome');
      return;
    }
    setStatus('Accepting welcome…');
    try {
      import_key_package_bundle(state.identityId, keyPackageStore.bundle);
    } catch (err) {
      console.warn('import_key_package_bundle failed (may be cached)', err);
    }
    const acceptResp = accept_welcome(state.identityId, welcome) as {
      group_id_hex: string;
    };
    state.groupIdHex = acceptResp.group_id_hex || groupIdHex;
    await updateGroupMetadata();
    maybeFinishHandshake();
  }

  async function updateGroupMetadata() {
    if (!state.groupIdHex) return;
    const groups = list_groups(state.identityId) as Array<{
      group_id_hex: string;
      nostr_group_id: string;
    }>;
    const group = groups.find((g) => g.group_id_hex === state.groupIdHex);
    const base = MoqAny.Path.from(state.session, 'wrappers');
    state.broadcastBase = base;
    state.publishPath = MoqAny.Path.join(base, state.role);
    state.subscribePath = MoqAny.Path.join(base, state.peerRole);
  }

  async function sendWelcome(precomputed?: string) {
    if (!state.groupIdHex || role !== 'alice') return;
    let welcome = precomputed;
    if (!welcome) {
      const config = {
        name: 'Marmot Chat',
        description: 'MoQ/MLS demo',
        relays: [relayRelaysUrl(relayUrl)],
        admins: [state.identityPub, keyPackageStore.pubkey!],
      };
      const members = [keyPackageStore.event!];
      const resp = create_group(state.identityId, config, members) as { welcome: string[] };
      welcome = resp.welcome[0];
    }
    if (!welcome) return;
    await publishHandshake({ type: 'welcome', welcome, groupIdHex: state.groupIdHex });
  }

  function maybeFinishHandshake() {
    if (!state.groupIdHex) return;
    handshakeComplete = true;
    clearInterval(keyPackageInterval);
    clearInterval(welcomeInterval);
    void connectMoq();
  }

  function relayRelaysUrl(url: URL) {
    const secure = url.protocol === 'https:' ? 'wss:' : 'wss:';
    return `${secure}//${url.host}`;
  }

  if (role === 'bob') {
    setStatus('Generating key package…');
    await sendBobKeyPackage();
    keyPackageInterval = window.setInterval(() => {
      void sendBobKeyPackage();
    }, HANDSHAKE_RETRY_MS);
    welcomeInterval = window.setInterval(() => {
      void publishHandshake({ type: 'request-welcome' });
    }, HANDSHAKE_RETRY_MS);
  } else {
    keyPackageInterval = window.setInterval(() => {
      void publishHandshake({ type: 'request-key-package' });
    }, HANDSHAKE_RETRY_MS);
    void publishHandshake({ type: 'request-key-package' });
  }

  async function connectMoq() {
    if (!state.groupIdHex || !state.publishPath || !state.subscribePath) return;
    setStatus('Connecting to relay…');

    try {
      const connection = await MoqAny.Connection.connect(new URL(relayUrl));
      state.connection = connection;
      setStatus('Connected to relay');

      const publisher = new MoqAny.Broadcast();
      connection.publish(state.publishPath, publisher);
      void processOutgoingRequests(publisher);

      void consumePeer(connection);

      window.chatReady = true;
      setStatus('Ready');
    } catch (err) {
      console.error('Failed to connect to relay', err);
      setStatus(`Relay connection failed: ${(err as Error).message}`);
    }
  }

  async function processOutgoingRequests(broadcast: any) {
    try {
      for (;;) {
        const request = await broadcast.requested();
        if (!request) break;
        const track = request.track as any;
        if (track.name !== TRACK_NAME) {
          console.warn('Unexpected track request', track.name);
          track.close();
          continue;
        }
        setStatus('Publisher track ready');
        state.outgoingTrack = track;
        state.outgoingDeferred.resolve(track);
        flushPending(track);
        try {
          await track.closed;
        } catch (err) {
          console.warn('Outgoing track closed with error', err);
        }
        state.outgoingDeferred = createDeferred<any>();
        state.outgoingTrack = undefined;
      }
    } catch (err) {
      console.error('Failed handling outgoing requests', err);
    }
  }

  async function consumePeer(connection: any) {
    if (!state.subscribePath) return;
    setStatus('Subscribing to peer…');
    for (;;) {
      try {
        const broadcast = connection.consume(state.subscribePath);
        const track = broadcast.subscribe(TRACK_NAME, 0);
        setStatus('Subscribed to peer');
        await readTrack(track);
      } catch (err) {
        console.warn('Subscribe failed, retrying', err);
        await delay(1000);
      }
    }
  }

  async function readTrack(track: any) {
    try {
      for (;;) {
        const frame = await track.readFrame();
        if (!frame) break;
        await handleWrapper(frame);
      }
    } catch (err) {
      console.warn('Track reader stopped', err);
    }
  }

  async function handleWrapper(frame: Uint8Array) {
    try {
      const result = ingest_wrapper(state.identityId, frame) as ProcessedWrapper;
      if (!result) return;
      console.debug('[marmot-chat] received wrapper', result.kind);
      if (result.kind === 'application' && result.message) {
        appendMessage({
          content: result.message.content,
          author: result.message.author,
          createdAt: result.message.created_at,
          local: result.message.author === state.identityPub,
        });
      } else if (result.kind === 'commit') {
        if (state.groupIdHex) {
          merge_pending_commit(state.identityId, state.groupIdHex);
        }
        state.chatState.commits += 1;
        renderSystemMessage('Merged commit');
      }
    } catch (err) {
      console.error('Failed to ingest wrapper', err);
    }
  }

  function flushPending(track: any) {
    while (pendingFrames.length > 0) {
      track.writeFrame(pendingFrames.shift()!);
    }
  }

  const pendingFrames: Uint8Array[] = [];

  async function sendWrapper(bytes: Uint8Array) {
    const track = state.outgoingTrack || (await state.outgoingDeferred.promise.catch(() => undefined));
    if (!track) {
      pendingFrames.push(bytes);
      return;
    }
    console.debug('[marmot-chat] sending frame', bytes.length);
    track.writeFrame(bytes);
  }

  async function sendMessage(content: string) {
    if (!content.trim()) return;
    if (!state.groupIdHex) throw new Error('Group not established');

    const rumor = {
      pubkey: state.identityPub,
      created_at: Math.floor(Date.now() / 1000),
      kind: 9,
      tags: [] as string[][],
      content,
    };
    const payload = { group_id_hex: state.groupIdHex, rumor };
    const bytes = create_message(state.identityId, payload) as Uint8Array;
    appendMessage({ content, author: state.identityPub, createdAt: rumor.created_at, local: true });
    await sendWrapper(bytes);
  }

  async function rotateCommit() {
    if (!state.groupIdHex) return;
    const result = self_update(state.identityId, state.groupIdHex) as {
      evolution_event: string;
    };
    const commitBytes = encoder.encode(result.evolution_event);
    try {
      ingest_wrapper(state.identityId, commitBytes);
      merge_pending_commit(state.identityId, state.groupIdHex);
    } catch (err) {
      console.warn('Failed to ingest local commit', err);
    }
    await sendWrapper(commitBytes);
    renderSystemMessage('Rotated epoch');
  }

  function appendMessage(message: ChatMessage) {
    state.chatState.messages.push(message);
    const item = document.createElement('div');
    item.className = `message${message.local ? ' message--self' : ''}`;

    const meta = document.createElement('div');
    meta.className = 'message__meta';
    const authorLabel = message.local ? 'You' : shortenKey(message.author);
    const time = new Date(message.createdAt * 1000).toLocaleTimeString();
    meta.textContent = `${authorLabel} · ${time}`;

    const content = document.createElement('div');
    content.className = 'message__content';
    content.textContent = message.content;

    item.append(meta, content);
    messagesEl.appendChild(item);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function renderSystemMessage(text: string) {
    const item = document.createElement('div');
    item.className = 'message';
    const meta = document.createElement('div');
    meta.className = 'message__meta';
    meta.textContent = 'System';
    const content = document.createElement('div');
    content.className = 'message__content';
    content.textContent = text;
    item.append(meta, content);
    messagesEl.appendChild(item);
    messagesEl.scrollTop = messagesEl.scrollHeight;
  }

  function shortenKey(key: string) {
    return `${key.slice(0, 6)}…${key.slice(-6)}`;
  }

  function setStatus(text: string) {
    statusEl.textContent = text;
  }

  formEl.addEventListener('submit', async (event) => {
    event.preventDefault();
    const content = messageInput.value.trim();
    if (!content) return;
    messageInput.value = '';
    const submitButton = formEl.querySelector('button[type="submit"]') as HTMLButtonElement | null;
    if (submitButton) submitButton.disabled = true;
    try {
      await sendMessage(content);
    } catch (err) {
      console.error('Failed to send message', err);
      renderSystemMessage(`Send failed: ${(err as Error).message}`);
    } finally {
      if (submitButton) submitButton.disabled = false;
    }
  });

  rotateBtn.addEventListener('click', () => {
    rotateBtn.disabled = true;
    void rotateCommit().finally(() => {
      rotateBtn.disabled = false;
    });
  });

  window.sendTestMessage = sendMessage;
  window.triggerCommit = rotateCommit;

  window.addEventListener('beforeunload', () => {
    clearInterval(keyPackageInterval);
    clearInterval(welcomeInterval);
    handshakeUnsubs.forEach((off) => off());
    handshake?.close();
    state.connection?.close();
  });
}

function showHelp(reason: string) {
  const dialogEl = document.getElementById('help');
  if (dialogEl && 'showModal' in dialogEl) {
    const dialog = dialogEl as HTMLDialogElement;
    const text = dialog.querySelector('p');
    if (text) {
      text.insertAdjacentHTML('beforeend', `<strong>${reason}</strong>`);
    }
    dialog.showModal();
    return;
  }
  alert(reason);
}

main().catch((err) => {
  console.error('Fatal error', err);
  const statusEl = document.getElementById('status');
  if (statusEl) statusEl.textContent = `Fatal error: ${(err as Error).message}`;
});
