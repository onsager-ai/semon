import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import http from 'node:http';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const types = { '.css': 'text/css', '.html': 'text/html', '.ico': 'image/x-icon', '.js': 'text/javascript', '.json': 'application/json', '.map': 'application/json', '.png': 'image/png', '.svg': 'image/svg+xml', '.woff2': 'font/woff2' };
export function storybookPolicy(html) {
  assert.ok(html.length <= 2 * 1024 * 1024, 'Storybook HTML bootstrap size exceeded');
  const hashes = [];
  for (const match of html.toString('utf8').matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)) {
    if (/(?:^|\s)src\s*=/i.test(match[1])) continue;
    const content = match[2].replace(/\r\n?/g, '\n');
    hashes.push(`'sha256-${createHash('sha256').update(content).digest('base64')}'`);
  }
  return `default-src 'self'; script-src 'self' ${[...new Set(hashes)].join(' ')}; style-src 'self' 'unsafe-inline'; img-src 'self' data:; font-src 'self'; connect-src 'self'; frame-src 'self'; object-src 'none'; base-uri 'none'`;
}
export function staticPath(raw) {
  assert.ok(typeof raw === 'string' && raw.length <= 8192 && raw.startsWith('/') && !raw.startsWith('//'));
  const decoded = decodeURIComponent(raw.split(/[?#]/, 1)[0]);
  assert.ok(!/[\\\u0000-\u0020\u007f%]/.test(decoded));
  assert.ok(!decoded.split('/').some((part) => part === '.' || part === '..'));
  return decoded === '/' ? '/index.html' : decoded;
}

/** Static capture uses the same consumer fixture handler as the workbench.
 * Missing assets are 404s; no navigation fallback or arbitrary filesystem read. */
export async function createStorybookServer(directory, options = {}) {
  const root = fs.realpathSync(directory);
  const assets = new Map();
  function collect(current) {
    for (const entry of fs.readdirSync(current, { withFileTypes: true })) {
      const file = path.join(current, entry.name);
      assert.equal(fs.realpathSync(file), file, 'Storybook static assets cannot contain symlinks');
      if (entry.isDirectory()) collect(file);
      else {
        assert.ok(entry.isFile() && fs.statSync(file).size <= 64 * 1024 * 1024);
        assert.ok(assets.size < 10000, 'Storybook asset inventory exceeded');
        assets.set('/' + path.relative(root, file).split(path.sep).join('/'), file);
      }
    }
  }
  collect(root);
  let fixtureHandler = options.fixtureHandler;
  const plugin = process.env.SEMON_STORYBOOK_FIXTURE_PLUGIN;
  if (plugin && !fixtureHandler) {
    assert.equal(process.env.GITHUB_ACTIONS, 'true', 'Consumer fixture plugins are CI-only');
    const consumer = fs.realpathSync(process.env.SEMON_STORYBOOK_CONSUMER_ROOT ?? '');
    assert.ok(path.isAbsolute(plugin) && plugin.startsWith(consumer + path.sep));
    assert.equal(fs.realpathSync(plugin), plugin);
    fixtureHandler = (await import(pathToFileURL(plugin).href)).createFixtureHandler();
  }
  const server = http.createServer(async (request, response) => {
    const reply = (status, body = '') => { response.writeHead(status, { 'Content-Type': 'text/plain', 'Cache-Control': 'no-store' }); response.end(body); };
    try {
      const pathname = staticPath(request.url);
      if (fixtureHandler && await fixtureHandler(request, response)) return;
      if (!['GET', 'HEAD'].includes(request.method)) { reply(405, 'Method refused'); return; }
      const asset = assets.get(pathname);
      if (!asset) { reply(404, 'Asset not found'); return; }
      assert.equal(fs.realpathSync(asset), asset);
      const stat = fs.statSync(asset);
      assert.ok(stat.isFile() && stat.size <= 64 * 1024 * 1024);
      const html = path.extname(asset) === '.html' ? fs.readFileSync(asset) : null;
      response.writeHead(200, { 'Content-Type': types[path.extname(asset)] ?? 'application/octet-stream', 'Content-Length': stat.size, 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...(html ? { 'Content-Security-Policy': storybookPolicy(html) } : {}) });
      if (request.method === 'HEAD') response.end();
      else if (html) response.end(html);
      else fs.createReadStream(asset, { flags: fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW }).on('error', () => response.destroy()).pipe(response);
    } catch { if (!response.headersSent) reply(400, 'Request refused'); else response.destroy(); }
  });
  server.requestTimeout = 10000;
  server.headersTimeout = 10000;
  await new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
  return { origin: `http://127.0.0.1:${server.address().port}`, close: () => new Promise((resolve, reject) => server.close((error) => error ? reject(error) : resolve())) };
}
