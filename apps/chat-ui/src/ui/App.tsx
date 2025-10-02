import { createSignal, createMemo, Show, Switch, Match, createEffect } from 'solid-js';
import { Onboarding, type OnboardingResult } from './Onboarding';
import { ChatView } from './ChatView';
import type { ChatSession } from '../types';
import { startChat } from '../chat/controller';
import { clearSession, loadSession } from '../utils';

export function App() {
  const defaults = typeof window !== 'undefined' ? window.__MARMOT_DEFAULTS ?? {} : {};
  const [session, setSession] = createSignal<ChatSession | null>(null);
  const [phase, setPhase] = createSignal<'onboarding' | 'chat'>('onboarding');

  const stored = createMemo<ChatSession | null>(() => {
    if (typeof window === 'undefined') return null;
    return loadSession();
  });

  createEffect(() => {
    const existing = stored();
    if (existing && phase() === 'onboarding') {
      setSession(existing);
      setPhase('chat');
    }
  });

  const handleOnboardingComplete = (result: OnboardingResult) => {
    setSession(result.session);
    setPhase('chat');
  };

  const handleReset = () => {
    clearSession();
    setSession(null);
    setPhase('onboarding');
  };

  return (
    <div class="app-layout">
      <Switch>
        <Match when={phase() === 'onboarding'}>
          <Onboarding onComplete={handleOnboardingComplete} defaults={defaults} />
        </Match>
        <Match when={phase() === 'chat'}>
          <Show when={session()} fallback={<div class="loading">Initializing…</div>}>
            {(value) => <ChatView session={value()} onReset={handleReset} startChat={startChat} />}
          </Show>
        </Match>
      </Switch>
    </div>
  );
}
