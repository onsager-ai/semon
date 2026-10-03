import { test } from 'node:test';
import assert from 'node:assert/strict';
import { checkDesign, readSources, sharedFiles, sharedPolicy, sharedRoot } from '../design-check.mjs';

const sources = readSources(sharedRoot, sharedFiles);
const check = css => checkDesign([...sources, { file: 'consumer.css', css }], sharedPolicy);
test('canonical CSS satisfies the design contract', () => assert.deepEqual(checkDesign(sources, sharedPolicy), []));
test('unknown tokens fail even with a CSS fallback', () => {
  assert.match(check('.title {font: 400 24px/1.2 var(--serif, sans-serif)}').join('\n'), /unresolved token --serif/);
  assert.deepEqual(check('.title {color: var(--ink)}'), []);
});
test('new color and typography literals require review', () => {
  for (const css of ['.x {color: #abcdef}', '.x {box-shadow: 0 0 3px rgb(1 2 3)}', '.x {color: red}', '.x {color: coral}', '.x {color: var(--ink, #abcdef)}', '.x {font-size: 19px}', '.x {font-family: Georgia}', '.x {font: 16px Georgia}']) assert.ok(check(css).length, css);
  assert.deepEqual(check('.x {font-size: var(--fs-cap); background: var(--sunken)}'), []);
});
test('faint text and undersized type fail; exact justified exceptions work', () => {
  assert.match(check('.x {color: var(--faint); font-size: 11px}').join('\n'), /below 12px/);
  const extra = { file: 'consumer.css', css: '.avatar {background: #6b7280; color: #fff}' };
  const exceptions = ['background|#6b7280', 'color|#fff'].map(value => ({ declaration: 'consumer.css|.avatar|' + value, reason: 'Matches the server avatar proxy.' }));
  assert.deepEqual(checkDesign([...sources, extra], sharedPolicy, { exceptions }), []);
  assert.ok(checkDesign(sources, sharedPolicy, { exceptions }).some(value => value.includes('Stale')));
  assert.ok(checkDesign([...sources, extra], sharedPolicy, { exceptions: exceptions.map(value => ({ ...value, reason: '' })) }).some(value => value.includes('no reason')));
});
test('comments and strings are not token references; nested var references are checked', () => {
  assert.deepEqual(check('/* var(--old) */ .x::before {content: "var(--old)"}'), []);
  assert.ok(check('.x {width: calc(var(--missing) + 1px)}').some(value => value.includes('--missing')));
});
test('consumers cannot override the shared palette or hide literals in local variables', () => {
  assert.ok(check(':root {--ink: #abcdef}').some(value => value.includes('must not redefine')));
  assert.ok(check(':root {--hub-color: coral}').some(value => value.includes('named color')));
  assert.deepEqual(check('.chart {--hub-size: 40px; width: var(--hub-size)}'), []);
});
