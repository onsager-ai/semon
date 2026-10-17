import { test } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import vm from 'node:vm';
import { overhaulPortReference } from './overhaul-port.mjs';

test('fixture data adaptation preserves the selected responsive session header', () => {
  const template = fs.readFileSync(new URL('./reference/overhaul.html', import.meta.url), 'utf8');
  const adapted = overhaulPortReference(
    { SESS: {}, TX: {}, H: [], MACHINE: {}, MACHINE_UP: {}, NOW: 0 },
    template,
  );
  const bar = (html) => html.match(/  function renderTopbar\([\s\S]*?\n  }\n/)[0];
  // The adapter's job is fixture units/data, not an independent header redesign.
  assert.equal(bar(adapted), bar(template));
  assert.doesNotMatch(adapted, /session-composer-trigger|t\.prepend\(.*Cached history/);
  assert.match(
    adapted,
    /bar\(s\.name, null, \{ session: s, lineage: lineage\.length > 1 \? lineage\.slice\(0, -1\)/,
  );
  const scripts = [...adapted.matchAll(/<script(?:\s[^>]*)?>([\s\S]*?)<\/script>/g)];
  assert.ok(scripts.length);
  for (const [, script] of scripts) new vm.Script(script);
});
