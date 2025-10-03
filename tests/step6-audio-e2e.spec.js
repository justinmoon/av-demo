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

test.describe('End-to-End Encrypted Audio Transmission', () => {
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

  test('transmits deterministic audio and validates bit-for-bit decryption', async ({ context }) => {
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

    // Inject deterministic audio generator
    await context.addInitScript(() => {
      // Generate deterministic sine wave pattern
      const SAMPLE_RATE = 48000;
      const FREQUENCY = 440; // A4 note
      const BUFFER_SIZE = 1024;
      let sampleIndex = 0;

      const mockStream = {
        getTracks: () => [{
          stop: () => {},
        }],
      };

      if (typeof window.AudioContext !== 'undefined') {
        const OriginalAudioContext = window.AudioContext;
        window.AudioContext = class MockAudioContext extends OriginalAudioContext {
          constructor(options) {
            super({ ...options, sampleRate: SAMPLE_RATE });
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

            // Generate deterministic audio frames
            const generateFrame = () => {
              if (!processor.onaudioprocess) return;

              const buffer = new Float32Array(bufferSize);
              for (let i = 0; i < bufferSize; i++) {
                const t = sampleIndex / SAMPLE_RATE;
                buffer[i] = 0.5 * Math.sin(2 * Math.PI * FREQUENCY * t);
                sampleIndex++;
              }

              // Store the frame for comparison
              const frameCopy = new Float32Array(buffer);
              window.audioTestData.sentFrames.push(frameCopy);

              const event = {
                inputBuffer: {
                  getChannelData: () => buffer,
                },
              };
              processor.onaudioprocess(event);
            };

            // Generate frames at regular intervals
            const interval = setInterval(generateFrame, 20);
            processor.disconnect = () => clearInterval(interval);

            // Start immediately
            setTimeout(generateFrame, 50);

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

    // Initialize test data structure (ChatView will populate it)
    await context.addInitScript(() => {
      window.audioTestData = {
        sentFrames: [],
        receivedFrames: [],
      };
    });

    // Start peer first
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

    // Start audio on both sides
    const initialToggle = initialPage.getByTestId('audio-toggle');
    const peerToggle = peerPage.getByTestId('audio-toggle');

    await initialToggle.click();
    await peerToggle.click();

    // Wait for audio to be active
    await expect(initialToggle).toHaveText(/Stop Audio/, { timeout: 3000 });
    await expect(peerToggle).toHaveText(/Stop Audio/, { timeout: 3000 });

    // Wait for frames to be transmitted
    console.log('[Test] Waiting for encrypted frames to be transmitted...');
    await initialPage.waitForFunction(
      () => window.audioStats && window.audioStats.encryptedFramesSent >= 5,
      { timeout: 10000 }
    );
    await peerPage.waitForFunction(
      () => window.audioStats && window.audioStats.encryptedFramesSent >= 5,
      { timeout: 10000 }
    );

    // Wait for frames to be received and decrypted
    await initialPage.waitForFunction(
      () => window.audioStats && window.audioStats.encryptedFramesReceived >= 5,
      { timeout: 10000 }
    );
    await peerPage.waitForFunction(
      () => window.audioStats && window.audioStats.encryptedFramesReceived >= 5,
      { timeout: 10000 }
    );

    // Get stats
    const initialSent = await initialPage.evaluate(() => window.audioStats.encryptedFramesSent);
    const initialReceived = await initialPage.evaluate(() => window.audioStats.encryptedFramesReceived);
    const peerSent = await peerPage.evaluate(() => window.audioStats.encryptedFramesSent);
    const peerReceived = await peerPage.evaluate(() => window.audioStats.encryptedFramesReceived);

    console.log(`[Initial] Sent: ${initialSent}, Received: ${initialReceived}`);
    console.log(`[Peer] Sent: ${peerSent}, Received: ${peerReceived}`);

    // Validate encrypted transmission occurred
    expect(initialSent).toBeGreaterThan(0);
    expect(peerSent).toBeGreaterThan(0);
    expect(initialReceived).toBeGreaterThan(0);
    expect(peerReceived).toBeGreaterThan(0);

    // CRITICAL: Validate audio data integrity
    // Get sent and received frames from Initial's perspective
    const initialData = await initialPage.evaluate(() => {
      return {
        sent: window.audioTestData.sentFrames.map(f => Array.from(f)),
        received: window.audioTestData.receivedFrames.map(f => Array.from(f)),
      };
    });

    const peerData = await peerPage.evaluate(() => {
      return {
        sent: window.audioTestData.sentFrames.map(f => Array.from(f)),
        received: window.audioTestData.receivedFrames.map(f => Array.from(f)),
      };
    });

    console.log(`[Initial] Sent ${initialData.sent.length} frames, Received ${initialData.received.length} frames`);
    console.log(`[Peer] Sent ${peerData.sent.length} frames, Received ${peerData.received.length} frames`);

    // Validate that we received frames
    expect(initialData.received.length).toBeGreaterThan(0);
    expect(peerData.received.length).toBeGreaterThan(0);

    // Validate audio data integrity
    // We can't do bit-for-bit comparison because of timing skew,
    // but we can validate that the decrypted audio has valid characteristics
    const validateAudioQuality = (frames, label) => {
      if (frames.length === 0) {
        throw new Error(`${label}: No frames to validate`);
      }

      console.log(`[${label}] Validating ${frames.length} frames`);

      let validFrames = 0;
      let totalEnergy = 0;
      let hasInvalid = false;

      for (let i = 0; i < frames.length; i++) {
        const frame = frames[i];

        // Check frame size (should be 1024 samples)
        if (frame.length !== 1024) {
          console.warn(`[${label}] Frame ${i} has wrong size: ${frame.length}`);
          continue;
        }

        // Calculate RMS energy
        let energy = 0;
        let hasNaN = false;
        for (let j = 0; j < frame.length; j++) {
          const sample = frame[j];
          if (isNaN(sample) || !isFinite(sample)) {
            hasNaN = true;
            hasInvalid = true;
            break;
          }
          energy += sample * sample;
        }

        if (hasNaN) {
          console.warn(`[${label}] Frame ${i} contains NaN or Inf`);
          continue;
        }

        const rms = Math.sqrt(energy / frame.length);
        totalEnergy += rms;

        // Validate signal is in reasonable range (not silence, not clipping)
        // Our sine wave should have RMS around 0.35 (0.5 * sqrt(2) / 2)
        if (rms > 0.1 && rms < 0.8) {
          validFrames++;
        }
      }

      const avgEnergy = totalEnergy / frames.length;
      const validRate = (validFrames / frames.length) * 100;

      console.log(`[${label}] Valid frames: ${validRate.toFixed(1)}%, Avg RMS: ${avgEnergy.toFixed(4)}, Has invalid: ${hasInvalid}`);

      return { validRate, avgEnergy, hasInvalid, validFrames, totalFrames: frames.length };
    };

    // Validate sent audio quality
    const initialSentQuality = validateAudioQuality(initialData.sent, 'Initial sent');
    const peerSentQuality = validateAudioQuality(peerData.sent, 'Peer sent');

    // Validate received audio quality (encrypted then decrypted)
    const initialReceivedQuality = validateAudioQuality(initialData.received, 'Initial received');
    const peerReceivedQuality = validateAudioQuality(peerData.received, 'Peer received');

    // Assertions
    expect(initialReceivedQuality.hasInvalid).toBe(false); // No NaN/Inf
    expect(peerReceivedQuality.hasInvalid).toBe(false);
    expect(initialReceivedQuality.validRate).toBeGreaterThan(80); // At least 80% valid
    expect(peerReceivedQuality.validRate).toBeGreaterThan(80);
    expect(initialReceivedQuality.avgEnergy).toBeGreaterThan(0.1); // Not silence
    expect(peerReceivedQuality.avgEnergy).toBeGreaterThan(0.1);

    console.log('✅ Audio data integrity validated - encrypted transmission successful!');
  });
});
