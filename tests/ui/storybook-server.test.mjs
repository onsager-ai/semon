import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { createStorybookServer, staticPath } from './storybook-server.mjs';

function request(origin, url, method = 'GET') {
  return new Promise((resolve, reject) => {
    const target = new URL(origin);
    const req = http.request({ hostname: target.hostname, port: target.port, path: url, method }, (res) => {
      let body = ''; res.on('data', (chunk) => { body += chunk; });
      res.on('end', () => resolve({ status: res.statusCode, body, headers: res.headers }));
    });
    req.on('error', reject); req.end();
  });
}
test('static Storybook serving refuses traversal, malformed paths, writes and missing assets', async (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'storybook-server-'));
  t.after(() => fs.rmSync(root, { force: true, recursive: true }));
  fs.writeFileSync(path.join(root, 'index.html'), '<html>workbench</html>');
  const server = await createStorybookServer(root);
  t.after(() => server.close());
  const page = await request(server.origin, '/index.html');
  assert.equal(page.status, 200); assert.match(page.headers['content-security-policy'], /script-src 'self'/);
  assert.equal((await request(server.origin, '/missing.js')).status, 404);
  assert.equal((await request(server.origin, '/unknown-route')).status, 404);
  assert.equal((await request(server.origin, '/index.html', 'POST')).status, 405);
  assert.equal((await request(server.origin, '/index.html', 'HEAD')).body, '');
  for (const bad of ['/../index.html', '/%2e%2e/index.html', '/%252e%252e/index.html', '/%E0%A4%A', '/%00', '//outside.test/index.html']) {
    assert.throws(() => staticPath(bad));
    assert.equal((await request(server.origin, bad)).status, 400);
  }
  fs.renameSync(path.join(root, 'index.html'), path.join(root, 'old.html'));
  fs.symlinkSync(path.join(root, 'old.html'), path.join(root, 'index.html'));
  assert.equal((await request(server.origin, '/index.html')).status, 400);
});
