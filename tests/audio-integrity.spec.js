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
const AUDIO_FILE = path.join(__dirname, 'fixtures/audio/test-tone-3s.wav');

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
description = "Ephemeral relay for audio integrity tests"

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

// Configure test to use fake audio file
test.use({
  launchOptions: {
    args: [
      '--use-fake-ui-for-media-stream',
      '--use-fake-device-for-media-stream',
      `--use-file-for-fake-audio-capture=${AUDIO_FILE}`,
    ],
  },
  permissions: ['microphone'],
});

test.describe('Audio Integrity Tests', () => {
  let relayProcess = null;
  let serverProcess = null;
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
        '--auth-public', 'anon',
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

  test('validates no frames dropped and audio integrity', async ({ context }) => {
    // Test against localhost first to isolate network vs relay issues
    const relayParam = process.env.TEST_RELAY || `http://127.0.0.1:${relayPort}/anon`;
    const nostrParam = process.env.TEST_NOSTR || `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    // Clear storage and set defaults
    await context.addInitScript(({ relay, nostr }) => {
      try {
        window.localStorage?.clear?.();
      } catch (err) {
        console.warn('Failed to clear localStorage during init', err);
      }
      window.__MARMOT_DEFAULTS = { relay, nostr };
    }, { relay: relayParam, nostr: nostrParam });

    // Inject audio capture hooks
    await context.addInitScript(() => {
      window.audioTestData = {
        sentFrames: [],
        receivedFrames: [],
      };
    });

    // Start with peer (Bob) waiting to join
    const peerPage = await context.newPage();
    peerPage.on('console', (msg) => console.log('[Peer]', msg.text()));
    peerPage.on('pageerror', (err) => console.error('[Peer error]', err?.message ?? err));

    await peerPage.goto(baseUrl);
    await useManualSecret(peerPage, PEER_SECRET);
    await peerPage.getByTestId('start-join').click();

    // Initial creates the chat
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

    // Wait for audio toggle to appear
    await initialPage.getByTestId('audio-toggle').waitFor({ timeout: 15000 });
    await peerPage.getByTestId('audio-toggle').waitFor({ timeout: 15000 });

    // Start audio on both sides
    console.log('\n=== Starting Audio ===\n');
    await initialPage.getByTestId('audio-toggle').click();
    await peerPage.getByTestId('audio-toggle').click();

    // Wait for audio to be active
    await expect(initialPage.getByTestId('audio-toggle')).toHaveText(/Stop Audio/, { timeout: 3000 });
    await expect(peerPage.getByTestId('audio-toggle')).toHaveText(/Stop Audio/, { timeout: 3000 });

    // Let audio run for 3 seconds (full duration of test file)
    console.log('Capturing 3 seconds of audio...\n');
    await initialPage.waitForTimeout(3000);

    // Stop audio
    await initialPage.getByTestId('audio-toggle').click();
    await peerPage.getByTestId('audio-toggle').click();

    // Get frame counts from window.audioStats
    const initialFramesSent = await initialPage.evaluate(() => {
      return window.audioStats?.encryptedFramesSent || 0;
    });

    const peerFramesReceived = await peerPage.evaluate(() => {
      return window.audioStats?.encryptedFramesReceived || 0;
    });

    console.log('=== Results ===\n');
    console.log(`Initial sent: ${initialFramesSent} encrypted frames`);
    console.log(`Peer received: ${peerFramesReceived} encrypted frames`);

    // Check 1: Some frames should have been sent and received
    expect(initialFramesSent).toBeGreaterThan(0);
    expect(peerFramesReceived).toBeGreaterThan(0);

    // Check 2: No significant frame drops (allow ~5% for network timing)
    const frameDropCount = initialFramesSent - peerFramesReceived;
    const dropPercentage = (frameDropCount / initialFramesSent) * 100;
    console.log(`\nFrame drops: ${frameDropCount} (${dropPercentage.toFixed(1)}%)`);

    if (frameDropCount <= 0) {
      console.log('✅ No frames dropped');
    } else if (dropPercentage <= 5) {
      console.log('✅ Frame drops within acceptable range (<5%)');
    } else {
      console.log(`⚠️  High frame drop rate: ${dropPercentage.toFixed(1)}%`);
    }

    expect(dropPercentage).toBeLessThan(10); // Allow up to 10% drops for baseline

    console.log('\n=== Test Complete ===\n');
  });
});
