export type Role = 'alice' | 'bob';

export interface ChatSession {
  role: Role;
  relay: string;
  nostr: string;
  sessionId: string;
  secretHex: string;
  inviteePubkey?: string;
  groupIdHex?: string;
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

