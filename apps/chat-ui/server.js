#!/usr/bin/env node
const http = require('http');
const fs = require('fs');
const path = require('path');

const args = process.argv.slice(2);
let port = 8890;
for (let i = 0; i < args.length; i += 1) {
  if ((args[i] === '--port' || args[i] === '-p') && args[i + 1]) {
    port = Number(args[i + 1]);
    i += 1;
  }
}

const appRoot = path.join(__dirname);
const wasmRoot = path.join(__dirname, '..', '..', 'tests', 'pkg');

const MIME_TYPES = {
  '.html': 'text/html',
  '.js': 'application/javascript',
  '.css': 'text/css',
  '.json': 'application/json',
  '.wasm': 'application/wasm',
  '.map': 'application/json',
};

function commonHeaders(contentType = 'text/plain') {
  return {
    'Content-Type': contentType,
    'Cross-Origin-Opener-Policy': 'same-origin',
    'Cross-Origin-Embedder-Policy': 'require-corp',
  };
}

function safeJoin(base, requestPath) {
  const resolved = path.normalize(path.join(base, requestPath));
  if (!resolved.startsWith(base)) {
    return null;
  }
  return resolved;
}

const server = http.createServer((req, res) => {
  if (!req.url) {
    res.writeHead(400, commonHeaders());
    res.end('Bad request');
    return;
  }

  const url = new URL(req.url, `http://localhost:${port}`);
  const pathname = decodeURIComponent(url.pathname);

  if (pathname === '/' || pathname === '') {
    serveFile(path.join(appRoot, 'index.html'), res);
    return;
  }

  if (pathname.startsWith('/tests/pkg/')) {
    const relative = pathname.replace('/tests/pkg/', '');
    const filePath = safeJoin(wasmRoot, relative);
    if (!filePath) {
      res.writeHead(403, commonHeaders());
      res.end('Forbidden');
      return;
    }
    serveFile(filePath, res);
    return;
  }

  // Try dist/ folder first (for built assets like main.js)
  const distPath = safeJoin(path.join(appRoot, 'dist'), pathname.slice(1));
  if (distPath && fs.existsSync(distPath)) {
    serveFile(distPath, res);
    return;
  }

  // Fall back to appRoot for other files
  const filePath = safeJoin(appRoot, pathname.slice(1));
  if (!filePath) {
    res.writeHead(403, commonHeaders());
    res.end('Forbidden');
    return;
  }

  serveFile(filePath, res);
});

function serveFile(filePath, res) {
  fs.readFile(filePath, (err, data) => {
    if (err) {
      res.writeHead(err.code === 'ENOENT' ? 404 : 500, commonHeaders());
      res.end('Not found');
      return;
    }
    const ext = path.extname(filePath);
    const type = MIME_TYPES[ext] || 'application/octet-stream';
    res.writeHead(200, commonHeaders(type));
    res.end(data);
  });
}

if (require.main === module) {
  server.listen(port, () => {
    console.log(`[chat-ui] listening at http://localhost:${port}/`);
  });
}

module.exports = server;
