import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/lib/navigation.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { projectShellNavigation, parseShellNavigation, withReturnPath } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
const input = () => ({
  leading: [{ key: 'new-session', label: 'New session', icon: 'M4 4h16', href: '/sessions/new' }],
  paths: { home: '/?compat=1', analytics: '/analytics?compat=1', machines: '/machines' },
  trailing: [
    {
      key: 'runtime-pending',
      label: 'Preparing session',
      icon: 'M4 5h16',
      href: '/runtimes/pending',
    },
  ],
});
test('all entry paths share destination order; selected context only changes the active state', () => {
  const policy = parseShellNavigation(input());
  for (const [route, active] of [
    ['sources', 'sessions'],
    ['sessions', 'sessions'],
    ['session', 'sessions'],
    ['trace', 'sessions'],
    ['new-session', 'new-session'],
    ['machines', 'machines'],
    ['runtime-pending', 'runtime-pending'],
    ['connections', undefined],
  ]) {
    const rows = projectShellNavigation(policy, route);
    assert.deepEqual(
      rows.map((row) => row.key),
      ['new-session', 'home', 'sessions', 'analytics', 'machines', 'runtime-pending'],
    );
    assert.equal(rows.find((row) => row.current)?.key, active);
    assert.equal(rows[1].href, '/?compat=1');
    assert.equal(rows[3].href, '/analytics?compat=1');
    assert(rows.every((row) => row.count === undefined));
  }
  const rows = projectShellNavigation(policy, 'session', {
    sessions: '/sessions?machine=one&q=task',
  });
  assert.equal(rows[2].href, '/sessions?machine=one&q=task');
});
test('host navigation cannot replace core identity or accept unsafe destinations', () => {
  for (const href of ['//foreign.test', 'javascript:alert(1)', '/\\foreign', '/\u0000bad']) {
    const policy = input();
    policy.leading[0].href = href;
    assert.equal(parseShellNavigation(policy), undefined);
  }
  const duplicate = input();
  duplicate.trailing[0].key = 'home';
  assert.equal(parseShellNavigation(duplicate), undefined);
  const repeated = input();
  repeated.trailing[0].key = 'new-session';
  assert.equal(parseShellNavigation(repeated), undefined);
});
test('return paths retain machine/filter context and refuse foreign navigation', () => {
  globalThis.location = { href: 'https://viewer.test/s/codex/session?machine=one' };
  const url = new URL(
    withReturnPath('/sessions/new', '/sessions?machine=one&q=retained#position'),
    location.href,
  );
  assert.equal(url.searchParams.get('return_to'), '/sessions?machine=one&q=retained');
  assert.equal(withReturnPath('/sessions/new', '//foreign.test'), '/sessions/new');
});
