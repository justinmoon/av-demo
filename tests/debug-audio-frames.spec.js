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
  const configPath = path.join(tmpDir, 'config.toml');
  const config = `relay_url = "wss://127.0.0.1:${port}/"
name = "test-relay"
description = "Test Nostr relay"

[info]
relay_url = "wss://127.0.0.1:${port}/"

[database]
data_directory = "${tmpDir}/db"

[network]
port = ${port}
address = "127.0.0.1"

[authorization]
pubkey_whitelist = []
`;
  fs.writeFileSync(configPath, config);
  return tmpDir;
}

// Custom test with browser launch options for fake audio
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

/**
 * Debug test for audio frame skipping issues
 *
 * Uses real audio file for testing, spawns real infrastructure.
 * Analyzes frame skip patterns to diagnose choppy audio.
 *
 * Run with: npx playwright test tests/debug-audio-frames.spec.js --headed
 */
test.skip('debug audio frame skipping with real infrastructure', async ({ page, context }) => {
  const testDuration = parseInt(process.env.DURATION_MS || '5000', 10);

  console.log('\n=== Audio Frame Skip Debug (Real Infrastructure) ===');
  console.log(`Duration: ${testDuration}ms`);
  console.log(`Audio file: ${AUDIO_FILE}\n`);

  // Ensure relay is built
  await ensureRelayBuilt();

  // Get free ports
  const relayPort = await getFreePort();
  const nostrPort = await getFreePort();
  const serverPort = await getFreePort();

  console.log(`Ports: relay=${relayPort}, nostr=${nostrPort}, server=${serverPort}`);

  // Start MoQ relay
  const relay = spawnProcess(RELAY_BIN, [
    '--listen',
    `127.0.0.1:${relayPort}`,
    '--tls-generate',
    'localhost,127.0.0.1',
    '--auth-public',
    'marmot',
    '--web-http-listen',
    `127.0.0.1:${relayPort}`,
  ]);

  // Start Nostr relay
  const tmpDir = createTempRelayConfig(nostrPort);
  const nostr = spawnProcess(NOSTR_BIN, ['-c', path.join(tmpDir, 'config.toml')]);

  // Wait for relays
  await waitForPort(relayPort);
  await waitForPort(nostrPort);

  // Start chat server
  const server = spawnProcess('node', [SERVER_BIN, '--port', serverPort]);
  await waitForPort(serverPort);

  const baseUrl = `http://127.0.0.1:${serverPort}`;
  const relayUrl = `https://127.0.0.1:${relayPort}/marmot`;
  const nostrUrl = `ws://127.0.0.1:${nostrPort}/`;

  try {
    // Track frame skips
    const peer1Skips = [];
    const peer2Skips = [];

    page.on('console', msg => {
      const text = msg.text();
      if (text.includes('[audio] Frame skip')) {
        const match = text.match(/expected (\d+) got (\d+)/);
        if (match) {
          peer1Skips.push({
            expected: parseInt(match[1]),
            got: parseInt(match[2]),
            gap: parseInt(match[2]) - parseInt(match[1]),
            time: Date.now(),
          });
          console.log('⚠️  [Peer 1]', text);
        }
      }
    });

    // Setup Peer 1 (initial)
    console.log('\n🔷 Setting up Peer 1...');
    await page.goto(baseUrl);
    await page.fill('[data-testid="manual-secret-input"]', INITIAL_SECRET);
    await page.click('[data-testid="manual-secret-continue"]');
    await page.waitForSelector('[data-testid="start-create"]');
    await page.click('[data-testid="start-create"]');

    // Enter peer details
    await page.fill('[data-testid="create-peer"]', PEER_PUB);
    await page.fill('[data-testid="create-relay"]', relayUrl);
    await page.fill('[data-testid="create-nostr"]', nostrUrl);
    await page.click('[data-testid="create-submit"]');

    // Get invite link
    const inviteLink = await page.inputValue('[data-testid="invite-link"]');
    await page.click('[data-testid="enter-chat"]');
    await page.waitForSelector('[data-testid="toggle-audio"]');
    console.log('🔷 Peer 1: In chat');

    // Setup Peer 2 (joiner)
    console.log('\n🔶 Setting up Peer 2...');
    const peer2 = await context.newPage();

    peer2.on('console', msg => {
      const text = msg.text();
      if (text.includes('[audio] Frame skip')) {
        const match = text.match(/expected (\d+) got (\d+)/);
        if (match) {
          peer2Skips.push({
            expected: parseInt(match[1]),
            got: parseInt(match[2]),
            gap: parseInt(match[2]) - parseInt(match[1]),
            time: Date.now(),
          });
          console.log('⚠️  [Peer 2]', text);
        }
      }
    });

    await peer2.goto(inviteLink);
    await peer2.fill('[data-testid="manual-secret-input"]', PEER_SECRET);
    await peer2.click('[data-testid="manual-secret-continue"]');
    await peer2.waitForSelector('[data-testid="join-submit"]');
    await peer2.click('[data-testid="join-submit"]');
    await peer2.waitForSelector('[data-testid="toggle-audio"]');
    console.log('🔶 Peer 2: Joined\n');

    // Start audio on both
    console.log('🎙️  Starting audio...\n');
    await page.click('[data-testid="toggle-audio"]');
    await peer2.click('[data-testid="toggle-audio"]');

    console.log(`⏱️  Running for ${testDuration/1000}s...\n`);
    await page.waitForTimeout(testDuration);

    console.log('🛑 Stopping audio...\n');
    await page.click('[data-testid="toggle-audio"]');
    await peer2.click('[data-testid="toggle-audio"]');

    // Analyze results
    console.log('=== Results ===\n');

    const analyzeSkips = (skips, peerName) => {
      if (skips.length === 0) {
        console.log(`${peerName}: ✅ No frame skips`);
        return;
      }

      const gaps = skips.map(s => s.gap);
      const avgGap = gaps.reduce((a, b) => a + b, 0) / gaps.length;
      const maxGap = Math.max(...gaps);
      const minGap = Math.min(...gaps);

      console.log(`${peerName}: ${skips.length} skips`);
      console.log(`  Gap range: ${minGap}-${maxGap} frames`);
      console.log(`  Average gap: ${avgGap.toFixed(2)} frames`);

      const allGapsOne = gaps.every(g => g === 1);
      if (allGapsOne) {
        console.log(`  ⚠️  All gaps = 1 frame (systematic issue)`);
      }

      if (skips.length > 1) {
        const intervals = [];
        for (let i = 1; i < skips.length; i++) {
          intervals.push(skips[i].time - skips[i-1].time);
        }
        const avgInterval = intervals.reduce((a, b) => a + b, 0) / intervals.length;
        console.log(`  Avg time between skips: ${avgInterval.toFixed(0)}ms`);
      }

      console.log(`  First ${Math.min(5, skips.length)} skips:`);
      skips.slice(0, 5).forEach(s => {
        console.log(`    Expected ${s.expected}, got ${s.got} (gap: ${s.gap})`);
      });
      console.log('');
    };

    analyzeSkips(peer1Skips, '🔷 Peer 1');
    analyzeSkips(peer2Skips, '🔶 Peer 2');

    // Diagnosis
    const totalSkips = peer1Skips.length + peer2Skips.length;
    if (totalSkips > 0) {
      console.log('=== Analysis ===\n');
      const allSkips = [...peer1Skips, ...peer2Skips];
      const allGapsOne = allSkips.every(s => s.gap === 1);

      if (allGapsOne) {
        console.log('🔍 DIAGNOSIS: Systematic single-frame skipping');
        console.log('Likely causes:');
        console.log('  - Frame counter logic error');
        console.log('  - Every other frame not delivered');
        console.log('  - MoQ publish/subscribe misalignment');
      } else {
        console.log('🔍 DIAGNOSIS: Random frame drops');
        console.log('Likely causes:');
        console.log('  - Network packet loss');
        console.log('  - MoQ relay dropping under load');
        console.log('  - Audio capture skipping frames');
        console.log('  - Processing too slow');
      }
    } else {
      console.log('✅ No frame skips - audio should be smooth\n');
    }
  } finally {
    // Cleanup
    relay.kill();
    nostr.kill();
    server.kill();
    fs.rmSync(tmpDir, { recursive: true, force: true });
  }

  console.log('=== Test Complete ===\n');
});
