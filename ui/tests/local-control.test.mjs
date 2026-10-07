import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
import { webcrypto } from 'node:crypto';
const { outputFiles } = await build({
  entryPoints: ['src/app/localControl.ts'],
  absWorkingDir: new URL('..', import.meta.url).pathname,
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { createLocalControl } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const model = () => ({
  thread: 'native',
  generation: 'generation',
  activeTurn: 'turn',
  connected: true,
  capabilities: {
    input: true,
    steer: true,
    interrupt: true,
    commandApproval: true,
    fileApproval: true,
    questions: true,
  },
  reason: null,
  requests: [],
  actions: {},
});
test('typed control owner binds exact targets and blocks blind replay after uncertain delivery', async () => {
  const oldFetch = globalThis.fetch,
    oldCrypto = globalThis.crypto;
  Object.defineProperty(globalThis, 'crypto', { value: webcrypto, configurable: true });
  const calls = [];
  globalThis.fetch = async (url, options) => {
    calls.push({ url, options, command: JSON.parse(options.body) });
    throw Error('transport lost');
  };
  const owner = createLocalControl(
    { request: () => new AbortController(), releaseRequest: () => {} },
    () => {},
  );
  try {
    owner.adopt(owner.prepare(model()));
    owner.view().send('hello');
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls.length, 1);
    assert.deepEqual(
      [calls[0].command.thread, calls[0].command.generation, calls[0].command.activeTurn],
      ['native', 'generation', 'turn'],
    );
    assert.equal(calls[0].options.credentials, 'same-origin');
    assert.equal(owner.view().uncertain, true);
    owner.adopt(owner.prepare({ ...model(), generation: 'next' }));
    owner.view().send('hello');
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(calls.length, 1);
    assert.equal(owner.view('another-session'), undefined);
    globalThis.fetch = async () => ({ ok: true, json: async () => ({ reconnected: true }) });
    owner.view().reconnect();
    await new Promise((resolve) => setImmediate(resolve));
    assert.equal(owner.view().uncertain, false);
  } finally {
    owner.destroy();
    globalThis.fetch = oldFetch;
    Object.defineProperty(globalThis, 'crypto', { value: oldCrypto, configurable: true });
  }
});
test('unsupported or malformed live models never create write capabilities', () => {
  const owner = createLocalControl({}, () => {});
  assert.equal(owner.prepare(undefined), null);
  for (const invalid of [
    {},
    { ...model(), activeTurn: 12 },
    { ...model(), capabilities: {} },
    { ...model(), requests: [{}] },
  ])
    assert.throws(() => owner.prepare(invalid));
});
test('follow-up delivery is confirmed before the composer may clear its draft', async () => {
  const oldFetch = globalThis.fetch;
  const calls = [];
  const owner = createLocalControl(
    { request: () => new AbortController(), releaseRequest: () => {} },
    () => {},
  );
  owner.adopt(owner.prepare({ ...model(), activeTurn: null }));
  try {
    globalThis.fetch = async (_url, options) => {
      const command = JSON.parse(options.body);
      calls.push(command);
      return {
        ok: true,
        json: async () => ({ snapshot: { actions: { [command.id]: { delivery: 'accepted' } } } }),
      };
    };
    assert.equal(await owner.view('native').send('First follow-up'), true);
    assert.equal(await owner.view('native').send('Second follow-up'), true);
    assert.equal(calls.length, 2);
    assert.notEqual(calls[0].id, calls[1].id);
    globalThis.fetch = async () => ({
      ok: true,
      json: async () => ({ snapshot: { actions: {} } }),
    });
    assert.equal(await owner.view().send('Unconfirmed'), false);
    assert.equal(owner.view().uncertain, true);
    assert.equal(await owner.view().send('Never replay this'), false);
  } finally {
    owner.destroy();
    globalThis.fetch = oldFetch;
  }
});

test('selection invalidates old control handles and late command receipts', async () => {
  const oldFetch = globalThis.fetch;
  let release;
  let writes = 0;
  globalThis.fetch = async () => {
    writes++;
    return new Promise((resolve) => {
      release = () => resolve({ ok: true, json: async () => ({}) });
    });
  };
  const owner = createLocalControl(
    { request: () => new AbortController(), releaseRequest() {} },
    () => {},
  );
  try {
    owner.observe(owner.prepare(model()));
    const old = owner.view('native');
    const pending = old.send('previous session');
    owner.observe(null);
    owner.observe(owner.prepare({ ...model(), thread: 'selected-next', generation: 'next' }));
    assert.equal(await old.send('never switch its destination'), false);
    assert.equal(writes, 1);
    release();
    assert.equal(await pending, false);
    assert.equal(owner.view('selected-next').busy, false);
    assert.equal(owner.view('selected-next').uncertain, false);
    assert.equal(owner.view('selected-next').note, '');
    owner.unavailable();
    assert.equal(owner.view('selected-next').snapshot.capabilities.input, false);
  } finally {
    owner.destroy();
    globalThis.fetch = oldFetch;
  }
});
