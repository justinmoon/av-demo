const { test, expect } = require('@playwright/test');
const { spawn } = require('child_process');
const fs = require('fs');
const path = require('path');
const net = require('net');
const os = require('os');
const { getPublicKey } = require('nostr-tools');
const { hexToBytes } = require('@noble/hashes/utils');

const CREATOR_SECRET = '4d36e7068b0eeef39b4e2ff1f908db8b27c12075b1219777084ffcf86490b6ae';
const INVITEE_SECRET = '6e8a52c9ac36ca5293b156d8af4d7f6aeb52208419bd99c75472fc6f4321a5fd';
const CREATOR_PUB = getPublicKey(hexToBytes(CREATOR_SECRET));
const INVITEE_PUB = getPublicKey(hexToBytes(INVITEE_SECRET));

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

async function waitForChatReady(page) {
  await page.waitForFunction(() => window.chatReady === true, null, { timeout: 20000 });
}

test.describe('Manual UI flow - 2 participants', () => {
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

    console.log(`[test] Starting MoQ relay on port ${relayPort}`);
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
    console.log(`[test] MoQ relay started`);

    console.log(`[test] Starting chat UI server on port ${serverPort}`);
    serverProcess = spawnProcess('node', [SERVER_BIN, '--port', String(serverPort)], {
      cwd: REPO_ROOT,
    });

    await waitForOutput(serverProcess.stdout, /listening at/, 2000);
    console.log(`[test] Chat UI server started`);
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

  test('two participants using dev secret flow', async ({ context }) => {
    await context.addInitScript(() => {
      try {
        window.localStorage?.clear?.();
      } catch (err) {
        console.warn('Failed to clear localStorage during init', err);
      }
    });

    const relayParam = `http://127.0.0.1:${relayPort}/marmot`;
    const nostrParam = `ws://127.0.0.1:${nostrPort}/`;
    const baseUrl = `http://127.0.0.1:${serverPort}`;

    console.log(`[test] MoQ relay URL: ${relayParam}`);
    console.log(`[test] Nostr relay URL: ${nostrParam}`);
    console.log(`[test] Chat UI URL: ${baseUrl}`);

    // Create initial (creator) page
    const initialPage = await context.newPage();
    initialPage.on('console', (msg) => console.log('[Creator]', msg.text()));
    initialPage.on('pageerror', (err) => console.error('[Creator error]', err?.message ?? err));
    await initialPage.goto(baseUrl);

    // Use dev secret to generate temp key
    await initialPage.getByTestId('use-dev-secret').click();
    await initialPage.waitForSelector('[data-testid="mode-pubkey"]', { timeout: 5000 });

    // Get creator's pubkey
    const creatorPubkey = await initialPage.getByTestId('mode-pubkey').inputValue();
    console.log(`[test] Creator pubkey: ${creatorPubkey}`);

    // Create invitee page
    const inviteePage = await context.newPage();
    inviteePage.on('console', (msg) => console.log('[Invitee]', msg.text()));
    inviteePage.on('pageerror', (err) => console.error('[Invitee error]', err?.message ?? err));
    await inviteePage.goto(baseUrl);

    // Use dev secret for invitee
    await inviteePage.getByTestId('use-dev-secret').click();
    await inviteePage.waitForSelector('[data-testid="mode-pubkey"]', { timeout: 5000 });

    // Get invitee's pubkey
    const inviteePubkey = await inviteePage.getByTestId('mode-pubkey').inputValue();
    console.log(`[test] Invitee pubkey: ${inviteePubkey}`);

    // Creator: Start create flow
    await initialPage.getByTestId('start-create').click();

    // Creator: Fill in peer pubkey and relay/nostr URLs
    await initialPage.getByTestId('create-peer').fill(inviteePubkey);
    await initialPage.getByTestId('create-relay').fill(relayParam);
    await initialPage.getByTestId('create-nostr').fill(nostrParam);
    await initialPage.getByTestId('create-submit').click();

    // Creator: Get invite link
    const inviteLink = await initialPage.getByTestId('invite-link').inputValue();
    console.log(`[test] Invite link: ${inviteLink}`);

    // Invitee: Go to join flow
    await inviteePage.getByTestId('start-join').click();

    // Invitee: Paste invite link and fill relay/nostr URLs
    await inviteePage.getByTestId('join-code').fill(inviteLink);
    await inviteePage.getByTestId('join-relay').fill(relayParam);
    await inviteePage.getByTestId('join-nostr').fill(nostrParam);

    // Both enter chat at same time
    await Promise.all([
      inviteePage.getByTestId('join-submit').click(),
      initialPage.getByTestId('enter-chat').click(),
    ]);

    // Wait for both to be ready
    console.log(`[test] Waiting for creator to be ready...`);
    await waitForChatReady(initialPage);
    console.log(`[test] Creator ready`);

    console.log(`[test] Waiting for invitee to be ready...`);
    await waitForChatReady(inviteePage);
    console.log(`[test] Invitee ready`);

    // Creator sends a message
    await initialPage.fill('#message', 'Hello from creator');
    await initialPage.click('#send-message');

    // Invitee should receive it
    await inviteePage.waitForFunction(
      () => window.chatState?.messages?.some((m) => m.content === 'Hello from creator' && !m.local),
      null,
      { timeout: 10000 }
    );

    // Invitee sends a message
    await inviteePage.fill('#message', 'Hello from invitee');
    await inviteePage.click('#send-message');

    // Creator should receive it
    await initialPage.waitForFunction(
      () => window.chatState?.messages?.some((m) => m.content === 'Hello from invitee' && !m.local),
      null,
      { timeout: 10000 }
    );

    console.log(`[test] Both participants can exchange messages successfully`);

    await Promise.all([initialPage.close(), inviteePage.close()]);
  });
});
