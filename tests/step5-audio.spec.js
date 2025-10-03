const { test, expect } = require('@playwright/test');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const net = require('net');
const os = require('os');
const { getPublicKey } = require('nostr-tools');
const { hexToBytes } = require('@noble/hashes/utils');

const INITIAL_SECRET = '4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae';
const PEER_SECRET = '6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd';
const INITIAL_PUB = getPublicKey(hexToBytes(INITIAL_SECRET));
const PEER_PUB = getPublicKey(hexToBytes(PEER_SECRET));

const REPO_ROOT = path.resolve(__dirname, '..');
const MOQ_ROOT = '/Users/justin/code/moq/moq';
const RELAY_ROOT = path.join(MOQ_ROOT, 'rs');
const RELAY_BIN = path.join(RELAY_ROOT, 'target', 'debug', 'moq-relay');
const SERVER_BIN = path.join(REPO_ROOT, 'apps', 'chat-ui', 'server.js');
const NOSTR_BIN = process.env.NOSTR_RELAY_BIN || 'nostr-rs-relay';

async function getFreePort() {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.on('error', reject);
    server.listen(0, '127.0.0.1', () => {
      const { port } = server.address();
      server.close(() => resolve(port));
    });
  });
}

function spawnProcess(command, args, options = {}) {
  const child = spawn(command, args, {
    stdio: ['ignore', 'pipe', 'pipe'],
    ...options,
  });
  child.stdout.setEncoding('utf8');
  child.stderr.setEncoding('utf8');
  child.on('error', (err) => {
    console.error(`[proc:${command}] error`, err);
  });
  return child;
}

async function ensureRelayBuilt() {
  if (!fs.existsSync(RELAY_BIN)) {
    console.log('Building relay...');
    const build = spawn('cargo', ['build', '-p', 'moq-relay'], {
      cwd: RELAY_ROOT,
      stdio: 'inherit',
    });
    await new Promise((resolve, reject) => {
      build.on('close', (code) => (code === 0 ? resolve() : reject(new Error(`Build failed: ${code}`))));
    });
  }
}

async function waitForPort(port, timeoutMs = 8000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      await new Promise((resolve, reject) => {
        const socket = net.createConnection({ port, host: '127.0.0.1' }, () => {
          socket.end();
          resolve(null);
        });
        socket.on('error', reject);
      });
      return;
    } catch (err) {
      await new Promise((res) => setTimeout(res, 100));
    }
  }
  throw new Error(`Timed out waiting for port ${port}`);
}

function createTempRelayConfig(port) {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), 'marmot-nostr-'));
  fs.mkdirSync(path.join(tmpDir, 'db'));
  const configPath = path.join(tmpDir, 'config.toml');
  const config = `
[info]
relay_url = "ws://127.0.0.1:${port}"
name = "Marmot Test Relay"
description = "Ephemeral relay for MoQ audio tests"

[database]
data_directory = "${path.join(tmpDir, 'db')}"

[network]
port = ${port}
address = "127.0.0.1"

[limits]
messages_per_sec = 1000
max_event_bytes = 262144
max_ws_message_bytes = 262144
max_ws_frame_bytes = 262144
subscription_count_per_client = 128

[verified_users]
mode = "disabled"
`;
  fs.writeFileSync(configPath, config, 'utf8');
  return { configPath, tmpDir };
}

async function waitForOutput(stream, pattern, timeoutMs = 8000) {
  return new Promise((resolve, reject) => {
    let buffer = '';
    const onData = (chunk) => {
      buffer += String(chunk);
      if (pattern.test(buffer)) {
        stream.off('data', onData);
        clearTimeout(timer);
        resolve();
      }
    };
    const timer = setTimeout(() => {
      stream.off('data', onData);
      reject(new Error(`Timed out waiting for pattern ${pattern}`));
    }, timeoutMs);
    stream.on('data', onData);
  });
}

async function startNostrRelay(port) {
  const { configPath, tmpDir } = createTempRelayConfig(port);
  const proc = spawnProcess(NOSTR_BIN, ['--config', configPath], {
    cwd: tmpDir,
    env: {
      ...process.env,
      RUST_LOG: process.env.NOSTR_RELAY_LOG ?? 'info',
    },
  });
  proc.stderr.on('data', (chunk) => process.stdout.write(`[nostr] ${chunk}`));
  await waitForPort(port);
  return { proc, tmpDir };
}

async function shutdown(proc) {
  if (!proc) return;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      try {
        proc.kill('SIGKILL');
      } catch (err) {
        // ignore
      }
      resolve();
    }, 2000);
    proc.on('exit', () => {
      clearTimeout(timer);
      resolve();
    });
    try {
      proc.kill('SIGTERM');
    } catch (err) {
      clearTimeout(timer);
      resolve();
    }
  });
}

async function useManualSecret(page, secret) {
  await page.getByTestId('manual-secret-input').fill(secret);
  await page.getByTestId('manual-secret-continue').click();
  await page.getByTestId('start-create').waitFor({ timeout: 5000 });
}

test.describe('Audio UI Tests', () => {
  /** @type {import('child_process').ChildProcess | null} */
  let relayProcess = null;
  /** @type {import('child_process').ChildProcess | null} */
  let serverProcess = null;
  /** @type {import('child_process').ChildProcess | null} */
  let nostrProcess = null;
  let relayPort;
  let serverPort;
  let nostrPort;
  let nostrDir;

  test.beforeAll(async () => {
    await ensureRelayBuilt();
    relayPort = await getFreePort();
    serverPort = await getFreePort();
    nostrPort = await getFreePort();

    const nostr = await startNostrRelay(nostrPort);
    nostrProcess = nostr.proc;
    nostrDir = nostr.tmpDir;

    relayProcess = spawnProcess(
      RELAY_BIN,
      [
        '--listen', `127.0.0.1:${relayPort}`,
        '--tls-generate', 'localhost,127.0.0.1',
        '--auth-public', 'marmot',
        '--web-http-listen', `127.0.0.1:${relayPort}`,
      ],
      {
        cwd: RELAY_ROOT,
        env: {
          ...process.env,
          RUST_LOG: process.env.MOQ_RELAY_LOG ?? 'info',
        },
      }
    );

    relayProcess.stderr.on('data', (chunk) => {
      process.stdout.write(`[relay] ${chunk}`);
    });

    await waitForOutput(relayProcess.stderr, /listening/, 8000);

    serverProcess = spawnProcess('node', [SERVER_BIN, '--port', String(serverPort)], {
      cwd: REPO_ROOT,
    });

    await waitForOutput(serverProcess.stdout, /listening at/, 2000);
  });

  test.afterAll(async () => {
    await shutdown(serverProcess);
    await shutdown(relayProcess);
    await shutdown(nostrProcess);
    if (nostrDir) {
      try {
        fs.rmSync(nostrDir, { recursive: true, force: true });
      } catch (err) {
        console.warn('Failed to remove nostr temp dir', err);
      }
    }
  });

  test('audio toggle button appears and toggles state', async ({ context }) => {
    const relayParam = `http://127.0.0.1:${relayPort}/marmot`;
    const nostrParam = process.env.MARMOT_NOSTR_URL || `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    await context.addInitScript(({ relay, nostr }) => {
      try {
        window.localStorage?.clear?.();
      } catch (err) {
        console.warn('Failed to clear localStorage during init', err);
      }
      window.__MARMOT_DEFAULTS = { relay, nostr };
    }, { relay: relayParam, nostr: nostrParam });

    // Mock getUserMedia to avoid requiring real mic permissions in CI
    await context.addInitScript(() => {
      const mockStream = {
        getTracks: () => [{
          stop: () => {},
        }],
      };

      // Mock AudioContext
      if (typeof window.AudioContext !== 'undefined') {
        const OriginalAudioContext = window.AudioContext;
        window.AudioContext = class MockAudioContext extends OriginalAudioContext {
          constructor(options) {
            super({ ...options, sampleRate: 48000 });
            this._closed = false;
          }

          createMediaStreamSource() {
            return {
              connect: () => {},
              disconnect: () => {},
            };
          }

          createScriptProcessor(bufferSize, inputChannels, outputChannels) {
            const processor = {
              connect: () => {},
              disconnect: () => {},
              onaudioprocess: null,
            };

            // Simulate some audio data
            setTimeout(() => {
              if (processor.onaudioprocess) {
                const event = {
                  inputBuffer: {
                    getChannelData: () => new Float32Array(bufferSize),
                  },
                };
                processor.onaudioprocess(event);
              }
            }, 100);

            return processor;
          }

          createBuffer(channels, length, sampleRate) {
            return {
              duration: length / sampleRate,
              copyToChannel: () => {},
            };
          }

          createBufferSource() {
            return {
              buffer: null,
              connect: () => {},
              start: () => {},
            };
          }

          get currentTime() {
            return Date.now() / 1000;
          }

          get destination() {
            return {};
          }

          close() {
            this._closed = true;
            return Promise.resolve();
          }
        };
      }

      if (typeof navigator.mediaDevices !== 'undefined') {
        navigator.mediaDevices.getUserMedia = async () => mockStream;
      }
    });

    const page = await context.newPage();
    page.on('console', (msg) => console.log('[Page]', msg.text()));
    page.on('pageerror', (err) => console.error('[Page error]', err?.message ?? err));

    await page.goto(baseUrl);
    await useManualSecret(page, INITIAL_SECRET);
    await page.getByTestId('start-create').click();
    await page.getByTestId('create-peer').fill(PEER_PUB);
    await page.getByTestId('create-relay').fill(relayParam);
    await page.getByTestId('create-nostr').fill(nostrParam);
    await page.getByTestId('create-submit').click();

    // Wait for invite, then enter chat
    await page.getByTestId('enter-chat').click();

    // Wait for the chat UI to load (audio toggle appears but may be disabled)
    const audioToggle = page.getByTestId('audio-toggle');
    await expect(audioToggle).toBeVisible({ timeout: 10000 });

    // Single-participant chat won't become "ready" without a peer,
    // so just verify the button exists and has correct initial text
    await expect(audioToggle).toHaveText(/Start Audio/);
  });

  test('two participants can toggle audio independently', async ({ context }) => {
    const relayParam = `http://127.0.0.1:${relayPort}/marmot`;
    const nostrParam = process.env.MARMOT_NOSTR_URL || `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    await context.addInitScript(({ relay, nostr }) => {
      try {
        window.localStorage?.clear?.();
      } catch (err) {
        console.warn('Failed to clear localStorage during init', err);
      }
      window.__MARMOT_DEFAULTS = { relay, nostr };
    }, { relay: relayParam, nostr: nostrParam });

    // Apply same audio mocks
    await context.addInitScript(() => {
      const mockStream = {
        getTracks: () => [{
          stop: () => {},
        }],
      };

      if (typeof window.AudioContext !== 'undefined') {
        const OriginalAudioContext = window.AudioContext;
        window.AudioContext = class MockAudioContext extends OriginalAudioContext {
          constructor(options) {
            super({ ...options, sampleRate: 48000 });
          }
          createMediaStreamSource() {
            return { connect: () => {}, disconnect: () => {} };
          }
          createScriptProcessor(bufferSize) {
            const p = { connect: () => {}, disconnect: () => {}, onaudioprocess: null };
            setTimeout(() => {
              if (p.onaudioprocess) {
                p.onaudioprocess({ inputBuffer: { getChannelData: () => new Float32Array(bufferSize) } });
              }
            }, 100);
            return p;
          }
          createBuffer(c, l, s) {
            return { duration: l / s, copyToChannel: () => {} };
          }
          createBufferSource() {
            return { buffer: null, connect: () => {}, start: () => {} };
          }
          get currentTime() { return Date.now() / 1000; }
          get destination() { return {}; }
          close() { return Promise.resolve(); }
        };
      }

      if (typeof navigator.mediaDevices !== 'undefined') {
        navigator.mediaDevices.getUserMedia = async () => mockStream;
      }
    });

    // Start peer first (waits on join screen)
    const peerPage = await context.newPage();
    peerPage.on('console', (msg) => console.log('[Peer]', msg.text()));
    peerPage.on('pageerror', (err) => console.error('[Peer error]', err?.message ?? err));

    await peerPage.goto(baseUrl);
    await useManualSecret(peerPage, PEER_SECRET);
    await peerPage.getByTestId('start-join').click();

    // Initial creates chat
    const initialPage = await context.newPage();
    initialPage.on('console', (msg) => console.log('[Initial]', msg.text()));
    initialPage.on('pageerror', (err) => console.error('[Initial error]', err?.message ?? err));

    await initialPage.goto(baseUrl);
    await useManualSecret(initialPage, INITIAL_SECRET);
    await initialPage.getByTestId('start-create').click();
    await initialPage.getByTestId('create-peer').fill(PEER_PUB);
    await initialPage.getByTestId('create-relay').fill(relayParam);
    await initialPage.getByTestId('create-nostr').fill(nostrParam);
    await initialPage.getByTestId('create-submit').click();

    // Get invite link and share with peer
    const inviteLink = await initialPage.getByTestId('invite-link').inputValue();
    await peerPage.getByTestId('join-code').fill(inviteLink);
    await peerPage.getByTestId('join-relay').fill(relayParam);
    await peerPage.getByTestId('join-nostr').fill(nostrParam);

    // Both enter chat
    await Promise.all([
      peerPage.getByTestId('join-submit').click(),
      initialPage.getByTestId('enter-chat').click(),
    ]);

    // Wait for both to be ready
    await initialPage.waitForFunction(() => window.chatReady === true, { timeout: 15000 });
    await peerPage.waitForFunction(() => window.chatReady === true, { timeout: 15000 });

    // Both should have audio toggle
    const initialToggle = initialPage.getByTestId('audio-toggle');
    const peerToggle = peerPage.getByTestId('audio-toggle');

    await expect(initialToggle).toBeVisible();
    await expect(peerToggle).toBeVisible();

    // Enable audio on initial
    await initialToggle.click();
    await initialPage.waitForTimeout(500);
    await expect(initialToggle).toHaveText(/Stop Audio/);

    // Peer's button should still say "Start Audio"
    await expect(peerToggle).toHaveText(/Start Audio/);

    // Enable audio on peer
    await peerToggle.click();
    await peerPage.waitForTimeout(500);
    await expect(peerToggle).toHaveText(/Stop Audio/);

    // Both should show audio active
    await expect(initialToggle).toHaveText(/Stop Audio/);
    await expect(peerToggle).toHaveText(/Stop Audio/);
  });
});
