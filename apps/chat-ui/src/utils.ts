import { hexToBytes } from '@noble/hashes/utils';
import type { ChatSession } from './types';

export const SESSION_STORAGE_KEY = 'marmot-chat-session-v1';

export { hexToBytes };

export function normalizeSecret(secret: string): string {
  if (!secret) {
    throw new Error('Secret key required');
  }
  const trimmed = secret.trim().toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]{64}$/.test(trimmed)) {
    throw new Error('Secret key must be 64 hex characters');
  }
  return trimmed;
}

export function normalizeHex(value: string, label = 'value'): string {
  const trimmed = value.trim().toLowerCase().replace(/^0x/, '');
  if (!/^[0-9a-f]{64}$/.test(trimmed)) {
    throw new Error(`${label} must be 64 hex characters`);
  }
  return trimmed;
}

export function randomHex(bytes = 32): string {
  const array = new Uint8Array(bytes);
  crypto.getRandomValues(array);
  return Array.from(array)
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('');
}

export function shortenKey(key: string, length = 6): string {
  if (!key) return '';
  if (key.length <= length * 2 + 1) return key;
  return `${key.slice(0, length)}…${key.slice(-length)}`;
}

export interface InvitePayload {
  session: string;
  relay: string;
  nostr: string;
}

export function parseInvite(raw: string): InvitePayload | null {
  if (!raw) return null;
  let text = raw.trim();
  if (!text) return null;
  try {
    if (text.startsWith('http')) {
      const url = new URL(text);
      const inviteParam = url.searchParams.get('invite');
      if (inviteParam) {
        text = decodeURIComponent(inviteParam);
      }
    }
  } catch (err) {
    // ignore
  }
  try {
    const parsed = JSON.parse(text);
    if (parsed && typeof parsed === 'object') {
      const session = String((parsed as any).session ?? '').trim();
      const relay = String((parsed as any).relay ?? '').trim();
      const nostr = String((parsed as any).nostr ?? '').trim();
      if (session && relay && nostr) {
        return { session, relay, nostr };
      }
    }
  } catch (err) {
    console.warn('Failed to parse invite payload', err);
  }
  return null;
}

export function persistSession(session: ChatSession) {
  try {
    localStorage.setItem(SESSION_STORAGE_KEY, JSON.stringify(session));
  } catch (err) {
    console.warn('Failed to persist session', err);
  }
}

export function loadSession(): ChatSession | null {
  try {
    const raw = localStorage.getItem(SESSION_STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as ChatSession;
    if (parsed.role !== 'initial' && parsed.role !== 'invitee') {
      localStorage.removeItem(SESSION_STORAGE_KEY);
      return null;
    }
    if (parsed.adminPubkeys && !Array.isArray(parsed.adminPubkeys)) {
      parsed.adminPubkeys = [];
    }
    if (!parsed.adminPubkeys) {
      parsed.adminPubkeys = [];
    }
    if (parsed.peerPubkeys && !Array.isArray(parsed.peerPubkeys)) {
      parsed.peerPubkeys = [];
    }
    if (!parsed.peerPubkeys) {
      parsed.peerPubkeys = [];
    }
    return parsed;
  } catch (err) {
    console.warn('Failed to load session', err);
    return null;
  }
}

export function clearSession() {
  try {
    localStorage.removeItem(SESSION_STORAGE_KEY);
  } catch (err) {
    console.warn('Failed to clear session', err);
  }
}
