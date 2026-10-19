import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { pathToFileURL } from 'node:url';

const uiRoot = decodeURIComponent(new URL('../', import.meta.url).pathname).replace(/\/$/, '');
const sessionSource = decodeURIComponent(
  new URL('../../crates/semon-sessions/src/', import.meta.url).pathname,
).replace(/\/$/, '');
const external = process.env.SEMON_STORYBOOK_EXTERNAL_STORY;
const consumerRoot = process.env.SEMON_STORYBOOK_CONSUMER_ROOT;
const fixturePlugin = process.env.SEMON_STORYBOOK_FIXTURE_PLUGIN;
function declaredFiles(name) {
  const raw = process.env[name];
  if (!raw) return [];
  assert.ok(raw.length <= 8192, `${name} is too large`);
  const files = JSON.parse(raw);
  assert.ok(Array.isArray(files) && files.length <= 8);
  assert.ok(files.every((file) => typeof file === 'string'));
  assert.equal(new Set(files).size, files.length);
  return files;
}
const externalStories = [...(external ? [external] : []), ...declaredFiles('SEMON_STORYBOOK_EXTERNAL_STORIES')];
const evidenceFiles = declaredFiles('SEMON_STORYBOOK_EXTERNAL_FILES');
const consumerFiles = [];
if (externalStories.length || evidenceFiles.length || fixturePlugin || consumerRoot) {
  assert.equal(process.env.GITHUB_ACTIONS, 'true', 'External synthetic stories are CI-only');
  assert.ok(consumerRoot && path.isAbsolute(consumerRoot));
  assert.equal(fs.realpathSync(consumerRoot), consumerRoot);
  assert.ok(externalStories.length > 0 && externalStories.length <= 8);
  assert.equal(new Set(externalStories).size, externalStories.length);
  for (const file of [...externalStories, fixturePlugin].filter(Boolean)) {
    assert.ok(path.isAbsolute(file) && file.startsWith(consumerRoot + path.sep));
    assert.equal(fs.realpathSync(file), file);
  }
  assert.ok(externalStories.every((file) => file.endsWith('.stories.tsx')));
  consumerFiles.push(consumerRoot);
  if (evidenceFiles.length) {
    const checkout = process.env.SEMON_STORYBOOK_CONSUMER_CHECKOUT;
    assert.ok(checkout && path.isAbsolute(checkout));
    assert.equal(fs.realpathSync(checkout), checkout);
    assert.ok(fs.existsSync(path.join(checkout, '.git')), 'Consumer must be an explicit checkout');
    assert.ok(consumerRoot.startsWith(checkout + path.sep));
    for (const file of evidenceFiles) {
      assert.ok(path.isAbsolute(file) && file.startsWith(checkout + path.sep));
      assert.equal(fs.realpathSync(file), file);
      assert.ok(file.endsWith('.json'), 'Only explicit evidence JSON is supported');
      const stat = fs.statSync(file);
      assert.ok(stat.isFile() && stat.size > 0 && stat.size <= 10 * 1024 * 1024);
      consumerFiles.push(file);
    }
  }
}
// These are the only public crate assets the workbench consumes. They are
// bundled/copied individually; the Rust source directory is never a staticDir.
const publicAssets = new Map([
  ['/fonts/instrument-sans-latin.woff2', 'fonts/instrument-sans/latin.woff2'],
  ['/fonts/instrument-sans-latin-ext.woff2', 'fonts/instrument-sans/latin-ext.woff2'],
  ['/fonts/jetbrains-mono-latin.woff2', 'fonts/jetbrains-mono/latin.woff2'],
  ['/fonts/jetbrains-mono-latin-ext.woff2', 'fonts/jetbrains-mono/latin-ext.woff2'],
  ['/mark.svg', 'mark.svg'],
]);
const sourceFiles = ['viewer.css', 'select.css', 'shell.css', ...publicAssets.values()].map(
  (file) => path.join(sessionSource, file),
);
// Official, individually imported harness artwork; never expose its directory.
sourceFiles.push(
  decodeURIComponent(new URL('../../assets/harnesses/codex-black.svg', import.meta.url).pathname),
  decodeURIComponent(new URL('../../assets/harnesses/codex.svg', import.meta.url).pathname),
);
function canonicalAssets() {
  return {
    name: 'semon-canonical-story-assets',
    enforce: 'pre',
    resolveId(id) {
      if (publicAssets.has(id)) return path.join(sessionSource, publicAssets.get(id));
    },
    generateBundle() {
      for (const [url, file] of publicAssets)
        this.emitFile({
          type: 'asset',
          fileName: url.slice(1),
          source: fs.readFileSync(path.join(sessionSource, file)),
        });
    },
    configureServer(server) {
      server.middlewares.use((request, response, next) => {
        const file = publicAssets.get(request.url?.split('?')[0]);
        if (!file) {
          next();
          return;
        }
        if (request.method !== 'GET') {
          response.writeHead(405);
          response.end();
          return;
        }
        response.writeHead(200, {
          'Content-Type': file.endsWith('.svg') ? 'image/svg+xml' : 'font/woff2',
          'Cache-Control': 'no-store',
        });
        response.end(fs.readFileSync(path.join(sessionSource, file)));
      });
    },
  };
}

export default {
  stories: ['../stories/**/*.stories.@(js|jsx|mjs|ts|tsx)', ...externalStories],
  framework: {
    name: '@storybook/preact-vite',
    options: {},
  },
  viteFinal: async (config) => {
    config.resolve ??= {};
    config.resolve.dedupe = [
      ...new Set([
        ...(config.resolve.dedupe ?? []),
        'preact',
        'preact/hooks',
        'preact/jsx-runtime',
      ]),
    ];
    config.plugins ??= [];
    config.plugins.push(canonicalAssets());
    if (fixturePlugin)
      config.plugins.push((await import(pathToFileURL(fixturePlugin).href)).default());
    config.server ??= {};
    config.server.fs ??= {};
    config.server.fs.allow = [
      ...new Set([...(config.server.fs.allow ?? []), uiRoot, ...sourceFiles, ...consumerFiles]),
    ];
    return config;
  },
};
