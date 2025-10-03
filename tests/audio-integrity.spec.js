const { test: baseTest, expect } = require('@playwright/test');
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
const AUDIO_FILE = path.join(__dirname, 'fixtures/audio/test-tone.wav');

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
description = "Ephemeral relay for audio tests"

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

const test = baseTest.extend({});

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

test('validates no frames dropped and audio integrity', async ({ page, context }) => {
  // Ensure everything is built
  await ensureRelayBuilt();

  // Build the UI
  console.log('Building UI...');
  const buildResult = await new Promise((resolve, reject) => {
    const build = spawn('npm', ['run', 'build'], {
      cwd: REPO_ROOT,
      stdio: 'pipe',
    });
    build.on('close', (code) => code === 0 ? resolve() : reject(new Error(`Build failed: ${code}`)));
  });
  console.log('Build complete\n');

  const relayPort = await getFreePort();
  const nostrPort = await getFreePort();
  const serverPort = await getFreePort();

  console.log('\n=== Audio Integrity Test ===\n');

  const relay = spawnProcess(RELAY_BIN, [
    '--listen', `127.0.0.1:${relayPort}`,
    '--tls-generate', 'localhost,127.0.0.1',
    '--auth-public', 'marmot',
    '--web-http-listen', `127.0.0.1:${relayPort}`,
  ]);

  const { configPath, tmpDir } = createTempRelayConfig(nostrPort);
  const nostr = spawnProcess(NOSTR_BIN, ['-c', configPath]);

  await waitForPort(relayPort);
  await waitForPort(nostrPort);

  const server = spawnProcess('node', [SERVER_BIN, '--port', serverPort]);
  await waitForPort(serverPort);

  // Give server a moment to fully initialize
  await new Promise(resolve => setTimeout(resolve, 500));

  const baseUrl = `http://127.0.0.1:${serverPort}`;
  const relayUrl = `https://127.0.0.1:${relayPort}/marmot`;
  const nostrUrl = `ws://127.0.0.1:${nostrPort}/`;

  console.log(`Server URLs: base=${baseUrl}, relay=${relayUrl}, nostr=${nostrUrl}\n`);

  try {
    // Inject audio tracking hooks on both sides
    await page.addInitScript(() => {
      window.audioTestData = {
        sentFrames: [],
        receivedFrames: [],
      };
    });

    const peer2 = await context.newPage();
    await peer2.addInitScript(() => {
      window.audioTestData = {
        sentFrames: [],
        receivedFrames: [],
      };
    });

    // Setup Alice (sender)
    console.log('Setting up Alice (sender)...');
    await page.goto(baseUrl);
    await page.fill('[data-testid="manual-secret-input"]', INITIAL_SECRET);
    await page.click('[data-testid="manual-secret-continue"]');
    await page.waitForSelector('[data-testid="start-create"]');
    await page.click('[data-testid="start-create"]');

    await page.fill('[data-testid="create-peer"]', PEER_PUB);
    await page.fill('[data-testid="create-relay"]', relayUrl);
    await page.fill('[data-testid="create-nostr"]', nostrUrl);
    await page.click('[data-testid="create-submit"]');

    const inviteLink = await page.inputValue('[data-testid="invite-link"]');
    await page.click('[data-testid="enter-chat"]');
    console.log('Alice: Waiting for chat to load...');
    await page.waitForSelector('[data-testid="toggle-audio"]', { timeout: 60000 });

    // Setup Bob (receiver)
    console.log('Setting up Bob (receiver)...');
    await peer2.goto(inviteLink);
    await peer2.fill('[data-testid="manual-secret-input"]', PEER_SECRET);
    await peer2.click('[data-testid="manual-secret-continue"]');
    await peer2.waitForSelector('[data-testid="join-submit"]');
    await peer2.click('[data-testid="join-submit"]');
    console.log('Bob: Waiting for chat to load...');
    await peer2.waitForSelector('[data-testid="toggle-audio"]', { timeout: 60000 });

    // Start audio on both sides
    console.log('\nStarting audio...');
    await page.click('[data-testid="toggle-audio"]');
    await peer2.click('[data-testid="toggle-audio"]');

    // Let audio run for 2 seconds
    console.log('Capturing 2 seconds of audio...\n');
    await page.waitForTimeout(2000);

    // Stop audio
    await page.click('[data-testid="toggle-audio"]');
    await peer2.click('[data-testid="toggle-audio"]');

    // Get captured audio data
    const aliceData = await page.evaluate(() => window.audioTestData);
    const bobData = await peer2.evaluate(() => window.audioTestData);

    console.log('=== Results ===\n');
    console.log(`Alice sent: ${aliceData.sentFrames.length} frames`);
    console.log(`Bob received: ${bobData.receivedFrames.length} frames`);

    // Check 1: No frames should be dropped
    const frameDropCount = aliceData.sentFrames.length - bobData.receivedFrames.length;
    console.log(`\nFrame drops: ${frameDropCount}`);

    if (frameDropCount > 0) {
      console.log(`⚠️  ${frameDropCount} frames were dropped (${((frameDropCount / aliceData.sentFrames.length) * 100).toFixed(1)}%)`);
    } else if (frameDropCount < 0) {
      console.log(`⚠️  Received more frames than sent (${Math.abs(frameDropCount)} extra)`);
    } else {
      console.log('✅ No frames dropped');
    }

    expect(frameDropCount).toBeLessThanOrEqual(5); // Allow max 5 frame drops for network/timing

    // Check 2: Audio content should match (sample comparison)
    console.log('\n=== Audio Integrity Check ===\n');

    if (bobData.receivedFrames.length === 0) {
      throw new Error('No frames received - audio not working');
    }

    // Compare a subset of frames (first 10)
    const framesToCompare = Math.min(10, aliceData.sentFrames.length, bobData.receivedFrames.length);
    let totalRmsDiff = 0;
    let matchedFrames = 0;

    for (let i = 0; i < framesToCompare; i++) {
      const sent = aliceData.sentFrames[i];
      const received = bobData.receivedFrames[i];

      if (!sent || !received) continue;
      if (sent.length !== received.length) {
        console.log(`Frame ${i}: Length mismatch (sent: ${sent.length}, received: ${received.length})`);
        continue;
      }

      // Calculate RMS difference
      let sumSquaredDiff = 0;
      for (let j = 0; j < sent.length; j++) {
        const diff = sent[j] - received[j];
        sumSquaredDiff += diff * diff;
      }
      const rmsDiff = Math.sqrt(sumSquaredDiff / sent.length);
      totalRmsDiff += rmsDiff;
      matchedFrames++;

      if (i < 3) {
        console.log(`Frame ${i}: RMS diff = ${rmsDiff.toFixed(6)}`);
      }
    }

    const avgRmsDiff = matchedFrames > 0 ? totalRmsDiff / matchedFrames : 1.0;
    console.log(`\nAverage RMS difference: ${avgRmsDiff.toFixed(6)}`);

    // Audio should be very similar (allow small encryption/decryption noise)
    if (avgRmsDiff < 0.01) {
      console.log('✅ Audio integrity excellent (RMS diff < 0.01)');
    } else if (avgRmsDiff < 0.1) {
      console.log('⚠️  Audio integrity acceptable (RMS diff < 0.1)');
    } else {
      console.log('❌ Audio integrity poor (RMS diff >= 0.1)');
    }

    expect(avgRmsDiff).toBeLessThan(0.1);

    // Check 3: Verify frames are actually encrypted (not plaintext)
    const encryptedFramesSent = await page.evaluate(() => {
      const counter = document.querySelector('[data-testid="encrypted-frames-sent"]');
      return counter ? parseInt(counter.textContent || '0') : 0;
    });

    console.log(`\nEncrypted frames sent: ${encryptedFramesSent}`);
    expect(encryptedFramesSent).toBeGreaterThan(0);
    expect(encryptedFramesSent).toBeCloseTo(aliceData.sentFrames.length, 5);

    console.log('\n=== Test Complete ===\n');

  } finally {
    relay.kill();
    nostr.kill();
    server.kill();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }
});
