import { createSignal, Show, Switch, Match } from 'solid-js';
import { Onboarding, type OnboardingResult } from './Onboarding';
import { ChatView } from './ChatView';
import type { ChatSession } from '../types';
import { startChat } from '../chat/controller';

export function App() {
  const defaults = typeof window !== 'undefined' ? window.__MARMOT_DEFAULTS ?? {} : {};
  const [session, setSession] = createSignal<ChatSession | null>(null);
  const [phase, setPhase] = createSignal<'onboarding' | 'chat'>('onboarding');

  const handleOnboardingComplete = (result: OnboardingResult) => {
    setSession(result.session);
    setPhase('chat');
  };

  return (
    <div class="app-layout">
      <Switch>
        <Match when={phase() === 'onboarding'}>
          <Onboarding onComplete={handleOnboardingComplete} defaults={defaults} />
        </Match>
        <Match when={phase() === 'chat'}>
          <Show when={session()} fallback={<div class="loading">Initializing…</div>}>
            {(value) => <ChatView session={value()} startChat={startChat} />}
          </Show>
        </Match>
      </Switch>
    </div>
  );
}
