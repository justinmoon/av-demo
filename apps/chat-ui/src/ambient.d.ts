declare module '../../../../tests/pkg/marmot_chat.js';

declare global {
  interface Window {
    __MARMOT_DEFAULTS?: {
      relay?: string;
      nostr?: string;
    };
  }
}

export {};
