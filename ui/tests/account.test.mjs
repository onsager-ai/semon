import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: ['src/lib/account.ts'],
  absWorkingDir: new URL('..', import.meta.url).pathname,
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { parseAccount } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const valid = () => ({
  name: 'Ada',
  login: '',
  initials: 'A',
  workspaces: [{ name: 'Bridge', role: 'owner', current: true, switch_href: '/switch' }],
  links: [{ label: 'Profile', href: '/profile', method: 'get', danger: false }],
});
test('defensive copy, getter snapshot, Unicode and safe paths', () => {
  const input = valid(),
    copy = parseAccount(input);
  input.workspaces[0].name = 'changed';
  assert.equal(copy.workspaces[0].name, 'Bridge');
  let reads = 0;
  assert.equal(
    parseAccount({
      ...valid(),
      get name() {
        reads++;
        return reads === 1 ? 'First' : 'Second';
      },
    }).name,
    'First',
  );
  assert.equal(reads, 1);
  assert.ok(parseAccount({ ...valid(), name: '😀'.repeat(80) }));
  for (const href of [
    '//evil.invalid',
    'javascript:alert(1)',
    '/\\evil',
    '/\u0000bad',
    '/\u0080bad',
    '/' + 'x'.repeat(512),
  ]) {
    const account = valid();
    account.links[0].href = href;
    assert.equal(parseAccount(account), null);
  }
  assert.equal(
    parseAccount({
      ...valid(),
      get name() {
        throw Error('hostile');
      },
    }),
    null,
  );
  assert.equal(
    parseAccount({ ...valid(), workspaces: Array(51).fill(valid().workspaces[0]) }),
    null,
  );
});
test('hostile markup remains text in the validated contract', () => {
  const markup = '<img src=x onerror=alert(1)>';
  assert.equal(parseAccount({ ...valid(), name: markup }).name, markup);
});
