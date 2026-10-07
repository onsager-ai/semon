import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/model.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { requestJson, ApiError } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].text).toString('base64')
);
test('failed JSON reads release an unread body while retaining actionable HTTP status', async () => {
  const original = globalThis.fetch;
  try {
    let canceled = 0;
    globalThis.fetch = async () =>
      new Response(
        new ReadableStream({
          cancel() {
            canceled++;
          },
        }),
        { status: 503, statusText: 'Unavailable' },
      );
    await assert.rejects(
      requestJson('/read'),
      (error) => error instanceof ApiError && error.status === 503,
    );
    assert.equal(canceled, 1);
    globalThis.fetch = async () => ({
      ok: false,
      status: 403,
      statusText: 'Forbidden',
      body: {
        cancel() {
          return Promise.reject(Error('Already retired'));
        },
      },
    });
    await assert.rejects(
      requestJson('/read'),
      (error) => error instanceof ApiError && error.status === 403,
    );
    globalThis.fetch = async () =>
      new Response(JSON.stringify({ accepted: true }), { status: 200 });
    assert.deepEqual(await requestJson('/read'), { accepted: true });
  } finally {
    globalThis.fetch = original;
  }
});
