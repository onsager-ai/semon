import { test } from 'node:test';
import assert from 'node:assert/strict';
import { build } from 'esbuild';
const { outputFiles } = await build({
  entryPoints: [new URL('../src/navigation/routes.ts', import.meta.url).pathname],
  bundle: true,
  write: false,
  format: 'esm',
  platform: 'node',
});
const { NavigationController } = await import(
  'data:text/javascript;base64,' + Buffer.from(outputFiles[0].contents).toString('base64')
);
test('route observers receive the selected canonical source destination and unsubscribe', () => {
  const navigation = new NavigationController();
  const observed = [];
  const unsubscribe = navigation.subscribeRoute(() => observed.push(navigation.route));
  navigation.route = { v: 'session', id: 'qualified-catalog-key' };
  navigation.route = { v: 'trace', sid: 'qualified-catalog-key', turn: 'recorded-turn' };
  unsubscribe();
  navigation.route = { v: 'home' };
  assert.deepEqual(observed, [
    { v: 'session', id: 'qualified-catalog-key' },
    { v: 'trace', sid: 'qualified-catalog-key', turn: 'recorded-turn' },
  ]);
});
test('destruction prevents a detached route owner from notifying or changing selection', () => {
  const navigation = new NavigationController();
  navigation.route = { v: 'analytics' };
  let notifications = 0;
  navigation.subscribeRoute(() => notifications++);
  navigation.destroy();
  navigation.subscribeRoute(() => notifications++);
  navigation.route = { v: 'session', id: 'late-result' };
  assert.equal(notifications, 0);
  assert.deepEqual(navigation.route, { v: 'analytics' });
});
