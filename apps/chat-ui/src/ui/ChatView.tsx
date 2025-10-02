import { createEffect, createSignal, onCleanup } from 'solid-js';
import { createStore } from 'solid-js/store';
import { For } from 'solid-js';
import type { ChatSession, ChatMessage, ChatState } from '../types';
import type { ChatHandle, ChatCallbacks } from '../chat/controller';

export interface ChatViewProps {
  session: ChatSession;
  onReset: () => void;
  startChat: (session: ChatSession, callbacks: ChatCallbacks) => Promise<ChatHandle>;
}

export type { ChatSession };

export function ChatView(props: ChatViewProps) {
  const [status, setStatus] = createSignal('Initializing…');
  const [chatState, setChatState] = createStore<ChatState>({ messages: [], commits: 0 });
  const [ready, setReady] = createSignal(false);
  const [sending, setSending] = createSignal(false);

  let controller: ChatHandle | null = null;
  let runId = 0;
  let messageInput: HTMLTextAreaElement | undefined;

  const syncWindowState = () => {
    if (typeof window === 'undefined') {
      return;
    }
    const snapshot: ChatState = {
      messages: chatState.messages.map((message) => ({ ...message })),
      commits: chatState.commits,
    };
    (window as any).chatState = snapshot;
    (window as any).chatReady = ready();
    (window as any).chatStatus = status();
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

  createEffect(() => {
    const session = props.session;
    if (!session) return;
    const currentRun = ++runId;

    stopController();
    setStatus('Initializing…');
    setChatState({ messages: [], commits: 0 });
    setReady(false);
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

  return (
    <main class="chat-app" id="chat-view-root">
      <header class="chat-app__header">
        <h1>Marmot Chat</h1>
        <div id="status" class="status">{status()}</div>
        <div class="info">
          <span id="role">Role: {formatRole(props.session.role)}</span>
          <span id="relay">Relay: {props.session.relay}</span>
          <span id="nostr">Nostr: {props.session.nostr}</span>
          <button
            type="button"
            id="reset-session"
            class="info__reset"
            onClick={() => {
              stopController();
              props.onReset();
            }}
            data-testid="reset-session"
          >
            Reset
          </button>
        </div>
      </header>

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
          <button type="submit" disabled={sending() || !ready()}>
            Send
          </button>
          <button type="button" id="rotate" onClick={handleRotate} disabled={sending() || !ready()}>
            Rotate Epoch
          </button>
        </form>
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
    case 'creator':
      return 'Creator';
    case 'joiner':
      return 'Joiner';
    default:
      return role;
  }
}
