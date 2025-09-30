const { test, expect } = require('@playwright/test');

let server;

test.beforeAll(async () => {
  // Start test server
  server = require('./server.js');
  // Wait for server to start
  await new Promise(resolve => setTimeout(resolve, 500));
});

test.afterAll(async () => {
  if (server) {
    server.close();
  }
});

test.describe('Phase 1 Step 2 - MLS chat demo (no MoQ)', () => {
  test('two pages exchange messages via BroadcastChannel with epoch rotation', async ({ context }) => {
    // Create two pages
    const pageA = await context.newPage();
    const pageB = await context.newPage();

    // Listen for console messages
    pageA.on('console', msg => {
      const text = msg.text();
      console.log('PageA:', text);
      // Check if we're sending plaintext
      if (text.includes('Test message') && text.includes('Broadcasting')) {
        console.error('⚠️  WARNING: Plaintext detected in broadcast!');
      }
    });
    pageB.on('console', msg => console.log('PageB:', msg.text()));

    // Listen for errors
    pageA.on('pageerror', err => console.error('PageA Error:', err));
    pageB.on('pageerror', err => console.error('PageB Error:', err));

    // Navigate to test pages
    await pageA.goto('http://localhost:8888/page-a.html');
    await pageB.goto('http://localhost:8888/page-b.html');

    // Wait for both pages to initialize
    await pageA.waitForFunction(() => window.pageAReady === true, { timeout: 10000 });
    await pageB.waitForFunction(() => window.pageBReady === true, { timeout: 10000 });

    console.log('Both pages initialized');

    // Test parameters
    const numMessages = 5;
    const messages = [];
    for (let i = 1; i <= numMessages; i++) {
      messages.push(`Test message ${i}`);
    }

    // Page A: Send application messages
    for (let i = 0; i < messages.length; i++) {
      console.log(`Sending message ${i + 1}: ${messages[i]}`);
      await pageA.evaluate((msg) => window.sendMessage(msg), messages[i]);
      // Small delay to ensure ordered delivery
      await new Promise(resolve => setTimeout(resolve, 100));
    }

    // Wait for Page B to receive all messages
    await pageB.waitForFunction(
      (count) => window.receivedMessages.length >= count,
      numMessages,
      { timeout: 5000 }
    );

    // Verify messages received correctly
    const receivedMessages = await pageB.evaluate(() => window.receivedMessages);
    console.log('Received messages:', receivedMessages);
    expect(receivedMessages).toHaveLength(numMessages);
    for (let i = 0; i < numMessages; i++) {
      expect(receivedMessages[i]).toBe(messages[i]);
    }

    // Page A: Send a self-update commit (epoch rotation)
    console.log('Sending epoch rotation commit');
    await pageA.evaluate(() => window.sendCommit());

    // Wait for Page B to process the commit
    await pageB.waitForFunction(
      () => window.receivedCommits >= 1,
      { timeout: 5000 }
    );

    const commitCount = await pageB.evaluate(() => window.receivedCommits);
    console.log('Commits processed:', commitCount);
    expect(commitCount).toBe(1);

    // Send one more message after epoch rotation
    console.log('Sending post-rotation message');
    await pageA.evaluate(() => window.sendMessage('Post-rotation message'));

    await pageB.waitForFunction(
      (count) => window.receivedMessages.length >= count,
      numMessages + 1,
      { timeout: 5000 }
    );

    const finalMessages = await pageB.evaluate(() => window.receivedMessages);
    console.log('Final messages:', finalMessages);
    expect(finalMessages).toHaveLength(numMessages + 1);
    expect(finalMessages[numMessages]).toBe('Post-rotation message');

    console.log('✓ Test passed: All messages delivered with epoch rotation');

    // Close pages
    await pageA.close();
    await pageB.close();
  });
});
