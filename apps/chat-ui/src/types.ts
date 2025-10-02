export type Role = 'initial' | 'invitee';

export interface ChatSession {
  role: Role;
  relay: string;
  nostr: string;
  sessionId: string;
  secretHex: string;
  groupIdHex?: string;
  adminPubkeys?: string[];
  peerPubkeys?: string[];
}

export interface ChatMessage {
  content: string;
  author: string;
  createdAt: number;
  local: boolean;
  system?: boolean;
}

export interface ChatMember {
  pubkey: string;
  isAdmin: boolean;
}

export interface ChatState {
  messages: ChatMessage[];
  commits: number;
  members: ChatMember[];
}
