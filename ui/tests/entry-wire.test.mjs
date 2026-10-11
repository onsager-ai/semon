import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  stdin: {
    contents: "export {parseEntry} from './src/state/transcript-wire';",
    resolveDir: new URL('..', import.meta.url).pathname,
  },
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseEntry } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const page = () => ({ entries: [] });
const parseTranscriptPage = (value) => ({
  entries: value.entries.map((entry) => parseEntry(entry)),
});
test('handoff entries retain validated image references, including masked prompts', () => {
  const reference = {
    o: 12,
    b: 1,
    v: '0123456789abcdef',
    w: 180,
    h: 390,
    type: 'image/png',
    size: 99,
  };
  const p = parseTranscriptPage({
    ...page(0, 1, 1),
    entries: [{ k: 'h', id: 'ask', img: [reference] }],
  });
  assert.deepEqual(p.entries[0].img[0], { ...reference, na: undefined });
});

test('unavailable attachment records omit a version while available images require it', () => {
  const masked = { o: 5738, b: 0, na: true };
  const p = parseTranscriptPage({
    ...page(0, 1, 1),
    entries: [{ k: 'h', id: 'ask', img: [masked] }],
  });
  assert.equal(p.entries[0].img[0].na, true);
  assert.equal(p.entries[0].img[0].v, undefined);
  assert.throws(
    () =>
      parseTranscriptPage({
        ...page(0, 1, 1),
        entries: [{ k: 'h', id: 'ask', img: [{ o: 1, b: 0 }] }],
      }),
    /Invalid/,
  );
});

test('numeric tool elapsed values normalize to display text at the wire boundary', () => {
  const p = parseTranscriptPage({
    ...page(0, 1, 1),
    entries: [{ k: 'tool', name: 'Read', arg: 'file', secs: 1 }],
  });
  assert.equal(p.entries[0].secs, '1');
});
