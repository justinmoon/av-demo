import initWasm, { WasmChatController } from '../../../../tests/pkg/marmot_chat.js';
import type { ChatMessage, ChatSession } from '../types';
import { createMoqBridge } from '../bridge/moq';

export interface ChatCallbacks {
  setStatus(text: string): void;
  pushMessage(message: ChatMessage): void;
  setCommits(total: number): void;
  setReady(ready: boolean): void;
}

export interface ChatHandle {
  stop(): void;
  sendMessage(content: string): void;
  rotate(): void;
}

interface ReadyState {
  ready: boolean;
}

let wasmReady: Promise<void> | null = null;

async function ensureWasm() {
  if (!wasmReady) {
    wasmReady = initWasm().then(() => undefined);
  }
  await wasmReady;
}

export async function startChat(session: ChatSession, callbacks: ChatCallbacks): Promise<ChatHandle> {
  await ensureWasm();
  await createMoqBridge();

  const eventHandler = (event: any) => {
    console.debug('[marmot-chat event]', event);
    switch (event.type) {
      case 'status':
        callbacks.setStatus(event.text ?? '');
        break;
      case 'ready':
        callbacks.setReady(Boolean((event as ReadyState).ready));
        break;
      case 'message': {
        const payload = event as { author: string; content: string; created_at: number; local?: boolean };
        const message: ChatMessage = {
          author: payload.author,
          content: payload.content,
          createdAt: payload.created_at,
          local: Boolean(payload.local),
        };
        callbacks.pushMessage(message);
        break;
      }
      case 'commit':
        callbacks.setCommits(Number(event.total ?? 0));
        break;
      case 'error':
        callbacks.setStatus(`Error: ${String(event.message ?? 'unknown')}`);
        callbacks.setReady(false);
        break;
      case 'handshake':
        callbacks.setStatus(`Handshake: ${event.phase ?? 'unknown'}`);
        break;
      default:
        console.debug('Unhandled chat event', event);
    }
  };

  const sessionValue = {
    role: session.role,
    relay_url: session.relay,
    nostr_url: session.nostr,
    session_id: session.sessionId,
    secret_hex: session.secretHex,
    invitee_pubkey: session.inviteePubkey,
    group_id_hex: session.groupIdHex,
    admin_pubkeys: session.adminPubkeys ?? [],
  };

  const controller = WasmChatController.start(sessionValue, eventHandler);

  return {
    stop: () => controller.shutdown(),
    sendMessage: (content: string) => controller.send_message(content),
    rotate: () => controller.rotate_epoch(),
  };
}
