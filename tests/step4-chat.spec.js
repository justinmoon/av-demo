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
const EXTRA_SECRET = '9c4e9aba1e3ff5deaa1bcb2a7dce1f2f4a5c6d7e8f9a0b1c2d3e4f5061728394';
const INITIAL_PUB = getPublicKey(hexToBytes(INITIAL_SECRET));
const PEER_PUB = getPublicKey(hexToBytes(PEER_SECRET));
const EXTRA_PUB = getPublicKey(hexToBytes(EXTRA_SECRET));

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

async function useManualSecret(page, secret) {
  await page.getByTestId('manual-secret-input').fill(secret);
  await page.getByTestId('manual-secret-continue').click();
  await page.getByTestId('start-create').waitFor({ timeout: 5000 });
}

async function waitForChatReady(page) {
  await page.waitForFunction(() => window.chatReady === true, null, { timeout: 20000 });
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

  test('three participants exchange messages over MoQ', async ({ context }) => {
    await context.addInitScript(() => {
      try {
        window.localStorage?.clear?.();
      } catch (err) {
        console.warn('Failed to clear localStorage during init', err);
      }
    });

    const relayParam = `http://127.0.0.1:${relayPort}/anon`;
    const nostrParam = process.env.MARMOT_NOSTR_URL || `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    const waitForMemberCount = async (page, expected) => {
      await page.waitForFunction(
        (expectedCount) => (window.chatState?.members?.length ?? 0) >= expectedCount,
        expected,
        { timeout: 15000 }
      );
    };

    const peerPage = await context.newPage();
    peerPage.on('console', (msg) => console.log('[Peer]', msg.text()));
    peerPage.on('pageerror', (err) => console.error('[Peer error]', err?.message ?? err, err?.error ?? '', err?.error?.stack ?? '', {
      type: err?.type,
      filename: err?.filename,
      lineno: err?.lineno,
      colno: err?.colno,
      error: err?.error,
    }));
    await peerPage.goto(baseUrl);
    await useManualSecret(peerPage, PEER_SECRET);
    await peerPage.getByTestId('start-join').click();

    const initialPage = await context.newPage();
    initialPage.on('console', (msg) => console.log('[Initial]', msg.text()));
    initialPage.on('pageerror', (err) => console.error('[Initial error]', err?.message ?? err, err?.error ?? '', err?.error?.stack ?? '', {
      type: err?.type,
      filename: err?.filename,
      lineno: err?.lineno,
      colno: err?.colno,
      error: err?.error,
    }));
    await initialPage.goto(baseUrl);
    await useManualSecret(initialPage, INITIAL_SECRET);
    await initialPage.getByTestId('start-create').click();
    await initialPage.getByTestId('create-peer').fill(PEER_PUB);
    await initialPage.getByTestId('create-relay').fill(relayParam);
    await initialPage.getByTestId('create-nostr').fill(nostrParam);
    await initialPage.getByTestId('create-submit').click();

    const inviteLink = await initialPage.getByTestId('invite-link').inputValue();

    await peerPage.getByTestId('join-code').fill(inviteLink);
    await peerPage.getByTestId('join-relay').fill(relayParam);
    await peerPage.getByTestId('join-nostr').fill(nostrParam);

    await Promise.all([
      peerPage.getByTestId('join-submit').click(),
      initialPage.getByTestId('enter-chat').click(),
    ]);

    await waitForChatReady(peerPage);
    await waitForChatReady(initialPage);

    await initialPage.getByTestId('invite-pubkey').fill(EXTRA_PUB);
    await initialPage.getByTestId('invite-submit').click();

    const extraPage = await context.newPage();
    extraPage.on('console', (msg) => console.log('[Extra]', msg.text()));
    extraPage.on('pageerror', (err) => console.error('[Extra error]', err?.message ?? err, err?.error ?? '', err?.error?.stack ?? '', {
      type: err?.type,
      filename: err?.filename,
      lineno: err?.lineno,
      colno: err?.colno,
      error: err?.error,
    }));
    await extraPage.goto(baseUrl);
    await useManualSecret(extraPage, EXTRA_SECRET);
    await extraPage.getByTestId('start-join').click();
    await extraPage.getByTestId('join-code').fill(inviteLink);
    await extraPage.getByTestId('join-relay').fill(relayParam);
    await extraPage.getByTestId('join-nostr').fill(nostrParam);
    await extraPage.getByTestId('join-submit').click();

    await waitForChatReady(extraPage);
    await initialPage.waitForFunction(
      () => typeof window.chatStatus === 'string' && window.chatStatus.includes('Invite ready'),
      null,
      { timeout: 15000 }
    );
    await waitForMemberCount(initialPage, 3);

    await initialPage.fill('#message', 'Hello everyone');
    await initialPage.click('#send-message');

    await Promise.all([
      peerPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Hello everyone' && !m.local),
        null,
        { timeout: 10000 }
      ),
      extraPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Hello everyone' && !m.local),
        null,
        { timeout: 10000 }
      ),
    ]);

    const extraRoster = await extraPage.evaluate(() => window.chatState?.members ?? []);
    expect(extraRoster.length).toBeGreaterThanOrEqual(2);

    await peerPage.fill('#message', 'Peer says hi');
    await peerPage.click('button[type="submit"]');

    await Promise.all([
      initialPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Peer says hi' && !m.local),
        null,
        { timeout: 10000 }
      ),
      extraPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Peer says hi' && !m.local),
        null,
        { timeout: 10000 }
      ),
    ]);

    await extraPage.fill('#message', 'Third participant online');
    await extraPage.click('button[type="submit"]');

    await Promise.all([
      initialPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Third participant online' && !m.local),
        null,
        { timeout: 10000 }
      ),
      peerPage.waitForFunction(
        () => window.chatState?.messages?.some((m) => m.content === 'Third participant online' && !m.local),
        null,
        { timeout: 10000 }
      ),
    ]);

    await Promise.all([initialPage.close(), peerPage.close(), extraPage.close()]);
  });
});
