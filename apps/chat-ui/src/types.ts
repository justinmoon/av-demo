export type Role = 'creator' | 'joiner';

export interface ChatSession {
  role: Role;
  relay: string;
  nostr: string;
  sessionId: string;
  secretHex: string;
  inviteePubkey?: string;
  groupIdHex?: string;
  adminPubkeys?: string[];
}

export interface ChatMessage {
  content: string;
  author: string;
  createdAt: number;
  local: boolean;
  system?: boolean;
}

export interface ChatState {
  messages: ChatMessage[];
  commits: number;
}
