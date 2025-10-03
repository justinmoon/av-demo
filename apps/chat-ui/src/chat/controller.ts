import initWasm, { WasmChatController } from '../../../../tests/pkg/marmot_chat.js';
import type { ChatMember, ChatMessage, ChatSession } from '../types';
import { createMoqBridge } from '../bridge/moq';

export type RecoveryAction = 'retry' | 'refresh' | 'check_connection' | 'none';

export interface ErrorInfo {
  message: string;
  fatal: boolean;
  recoveryAction?: RecoveryAction;
}

export interface ChatCallbacks {
  setStatus(text: string): void;
  pushMessage(message: ChatMessage): void;
  setCommits(total: number): void;
  setReady(ready: boolean): void;
  setRoster(members: ChatMember[]): void;
  upsertMember(member: ChatMember): void;
  removeMember(pubkey: string): void;
  showError(error: ErrorInfo): void;
  clearError(): void;
}

export interface ChatHandle {
  stop(): void;
  sendMessage(content: string): void;
  rotate(): void;
  invite(pubkey: string, isAdmin: boolean): void;
  // Media crypto methods
  deriveMediaBaseKey(senderPubkey: string, trackLabel: string): Promise<string>;
  encryptAudioFrame(baseKeyB64: string, plaintext: Uint8Array, frameCounter: number, aad: Uint8Array): Promise<Uint8Array>;
  decryptAudioFrame(baseKeyB64: string, ciphertext: Uint8Array, frameCounter: number, aad: Uint8Array): Promise<Uint8Array>;
  currentEpoch(): Promise<number>;
  groupRoot(): Promise<string>;
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

  const toMember = (raw: any): ChatMember | null => {
    if (!raw || typeof raw !== 'object') return null;
    const pubkey = typeof raw.pubkey === 'string' ? raw.pubkey : String(raw.pubkey ?? '');
    if (!pubkey) return null;
    return {
      pubkey,
      isAdmin: Boolean((raw as any).is_admin ?? (raw as any).isAdmin ?? false),
    };
  };

  const eventHandler = (event: any) => {
    console.debug('[marmot-chat event]', event);
    switch (event.type) {
      case 'status':
        callbacks.setStatus(event.text ?? '');
        break;
      case 'ready':
        callbacks.clearError();
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
      case 'roster': {
        const members = Array.isArray(event.members)
          ? (event.members as any[])
              .map((member) => toMember(member))
              .filter((member): member is ChatMember => member !== null)
          : [];
        callbacks.setRoster(members);
        break;
      }
      case 'member_joined':
      case 'member_updated': {
        const member = toMember((event as any).member);
        if (member) {
          callbacks.upsertMember(member);
        }
        break;
      }
      case 'member_left': {
        const pubkey = typeof event.pubkey === 'string' ? event.pubkey : '';
        if (pubkey) {
          callbacks.removeMember(pubkey);
        }
        break;
      }
      case 'invite_generated': {
        const recipient = typeof event.recipient === 'string' ? event.recipient : 'unknown recipient';
        const adminFlag = event.is_admin ? 'admin' : 'member';
        callbacks.setStatus(`Invite ready for ${recipient} (${adminFlag})`);
        break;
      }
      case 'error': {
        const errorEvent = event as { message: string; fatal?: boolean; recovery_action?: RecoveryAction };
        const fatal = errorEvent.fatal !== false; // Default to true if undefined
        const recoveryAction = errorEvent.recovery_action;

        callbacks.showError({
          message: errorEvent.message ?? 'Unknown error',
          fatal,
          recoveryAction,
        });

        if (fatal) {
          callbacks.setReady(false);
        }
        break;
      }
      case 'handshake':
        callbacks.setStatus(`Handshake: ${event.phase ?? 'unknown'}`);
        break;
      default:
        console.debug('Unhandled chat event', event);
    }
  };

  const sessionValue = {
    bootstrap_role: session.role,
    relay_url: session.relay,
    nostr_url: session.nostr,
    session_id: session.sessionId,
    secret_hex: session.secretHex,
    group_id_hex: session.groupIdHex,
    admin_pubkeys: session.adminPubkeys ?? [],
    peer_pubkeys: session.peerPubkeys ?? [],
  };

  const controller = WasmChatController.start(sessionValue, eventHandler);

  return {
    stop: () => controller.shutdown(),
    sendMessage: (content: string) => controller.send_message(content),
    rotate: () => controller.rotate_epoch(),
    invite: (pubkey: string, isAdmin: boolean) => controller.inviteMember(pubkey, isAdmin),
    // Media crypto methods
    deriveMediaBaseKey: async (senderPubkey: string, trackLabel: string) => {
      return controller.deriveMediaBaseKey(senderPubkey, trackLabel);
    },
    encryptAudioFrame: async (baseKeyB64: string, plaintext: Uint8Array, frameCounter: number, aad: Uint8Array) => {
      return controller.encryptAudioFrame(baseKeyB64, plaintext, frameCounter, aad);
    },
    decryptAudioFrame: async (baseKeyB64: string, ciphertext: Uint8Array, frameCounter: number, aad: Uint8Array) => {
      return controller.decryptAudioFrame(baseKeyB64, ciphertext, frameCounter, aad);
    },
    currentEpoch: async () => {
      return Number(controller.currentEpoch());
    },
    groupRoot: async () => {
      return controller.groupRoot();
    },
  };
}
