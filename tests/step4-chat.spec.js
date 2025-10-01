const { test, expect } = require('@playwright/test');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const net = require('net');
const os = require('os');

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
    await runCommand('cargo', ['build', '-p', 'moq-relay'], { cwd: RELAY_ROOT });
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
description = "Ephemeral relay for MoQ chat tests"

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
  return { tmpDir, configPath };
}

async function startNostrRelay(port) {
  const { tmpDir, configPath } = createTempRelayConfig(port);
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

function runCommand(command, args, options = {}) {
  return new Promise((resolve, reject) => {
    const child = spawn(command, args, { stdio: 'inherit', ...options });
    child.on('error', reject);
    child.on('exit', (code) => {
      if (code === 0) resolve();
      else reject(new Error(`${command} ${args.join(' ')} exited with code ${code}`));
    });
  });
}

async function waitForOutput(stream, regex, timeoutMs = 5000) {
  return new Promise((resolve, reject) => {
    let buffer = '';
    const timer = setTimeout(() => {
      cleanup();
      reject(new Error(`Timed out waiting for ${regex}`));
    }, timeoutMs);

    function onData(chunk) {
      buffer += chunk.toString();
      if (regex.test(buffer)) {
        cleanup();
        resolve();
      }
    }

    function onClose() {
      cleanup();
      reject(new Error(`Stream closed before matching ${regex}`));
    }

    function cleanup() {
      clearTimeout(timer);
      stream.off('data', onData);
      stream.off('close', onClose);
      stream.off('error', onError);
    }

    function onError(err) {
      cleanup();
      reject(err);
    }

    stream.on('data', onData);
    stream.once('close', onClose);
    stream.once('error', onError);
  });
}

async function shutdown(child) {
  if (!child || child.killed) return;
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      if (!child.killed) {
        child.kill('SIGKILL');
      }
    }, 3000);
    child.once('close', () => {
      clearTimeout(timer);
      resolve();
    });
    child.kill('SIGINT');
  });
}

test.describe('Phase 1 Step 4 - MoQ browser chat', () => {
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

  test('two browser tabs exchange messages over MoQ', async ({ context }) => {
    const sessionId = `pw-${Date.now().toString(16)}`;
    const relayParam = `http://127.0.0.1:${relayPort}/marmot`;
    const nostrParam = process.env.MARMOT_NOSTR_URL || `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    const bob = await context.newPage();
    bob.on('console', (msg) => console.log('[Bob]', msg.text()));
    bob.on('pageerror', (err) => console.error('[Bob error]', err));

    await bob.goto(
      `${baseUrl}/?role=bob&relay=${encodeURIComponent(relayParam)}&nostr=${encodeURIComponent(nostrParam)}&session=${sessionId}`
    );

    const alice = await context.newPage();
    alice.on('console', (msg) => console.log('[Alice]', msg.text()));
    alice.on('pageerror', (err) => console.error('[Alice error]', err));

    await alice.goto(
      `${baseUrl}/?role=alice&relay=${encodeURIComponent(relayParam)}&nostr=${encodeURIComponent(nostrParam)}&session=${sessionId}`
    );

    await bob.waitForFunction(() => window.chatReady === true, null, { timeout: 20000 });
    await alice.waitForFunction(() => window.chatReady === true, null, { timeout: 20000 });

    // Alice sends a message
    await alice.fill('#message', 'Hello Bob');
    await alice.click('button[type="submit"]');

    await bob.waitForFunction(
      () => window.chatState?.messages?.some((m) => m.content === 'Hello Bob'),
      null,
      { timeout: 10000 }
    );

    const bobMessages = await bob.evaluate(() => window.chatState?.messages ?? []);
    expect(bobMessages.map((m) => m.content)).toContain('Hello Bob');

    // Bob replies
    await bob.fill('#message', 'Hello Alice');
    await bob.click('button[type="submit"]');

    await alice.waitForFunction(
      () => window.chatState?.messages?.some((m) => m.content === 'Hello Alice' && !m.local),
      null,
      { timeout: 10000 }
    );

    const aliceMessages = await alice.evaluate(() => window.chatState?.messages ?? []);
    expect(aliceMessages.map((m) => m.content)).toContain('Hello Alice');

    // Rotate epoch from Alice
    await alice.click('#rotate');

    await bob.waitForFunction(
      () => (window.chatState?.commits ?? 0) >= 1,
      null,
      { timeout: 10000 }
    );

    // Exchange another message
    await alice.fill('#message', 'Post-commit ping');
    await alice.click('button[type="submit"]');

    await bob.waitForFunction(
      () => window.chatState?.messages?.some((m) => m.content === 'Post-commit ping'),
      null,
      { timeout: 10000 }
    );

    const finalBobMessages = await bob.evaluate(() => window.chatState?.messages ?? []);
    expect(finalBobMessages.map((m) => m.content)).toContain('Post-commit ping');

    await alice.close();
    await bob.close();
  });
});
