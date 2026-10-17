import assert from 'node:assert/strict';
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import vm from 'node:vm';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const source = process.env.SEMON_FIXTURE_DATA_SOURCE;
if (!/^[0-9a-f]{40}$/.test(source ?? ''))
  throw new Error('SEMON_FIXTURE_DATA_SOURCE must be a full pinned Git revision');

function originalData(name) {
  const html = execFileSync(
    'git',
    ['show', `${source}:tests/ui/reference/${name}.html`],
    { encoding: 'utf8', maxBuffer: 5 * 1024 * 1024 },
  );
  const start = html.indexOf('  const T = (h, m) => h * 60 + m;');
  const end = html.indexOf('  // ====================================================================================\n  const $ = ');
  if (start < 0 || end < start) throw new Error(`${name}: original data block is missing`);
  const values = vm.runInNewContext(
    '(() => {\n' +
      html.slice(start, end) +
      '\nreturn { NOW, MACHINE, MACHINE_UP, SESS, H, TX, API_PRICE };\n})()',
  );
  const historyStart = html.indexOf('  const ANALYTICS_HISTORY = {');
  const historyEnd = html.indexOf('  function analyticsSessions()', historyStart);
  if (historyStart < 0 || historyEnd < historyStart)
    throw new Error(`${name}: original Analytics data block is missing`);
  const history = vm.runInNewContext(
    '(() => { const T = (h, m) => h * 60 + m;\n' +
      html.slice(historyStart, historyEnd) +
      '\nreturn { ANALYTICS_HISTORY, ANALYTICS_WAIT_SAMPLES };\n})()',
  );
  return { ...values, ...history };
}

for (const name of ['overhaul', 'semon-sample']) {
  const expected = JSON.parse(JSON.stringify(originalData(name)));
  const actual = JSON.parse(
    fs.readFileSync(path.join(here, 'fixture-data', name + '.json'), 'utf8'),
  );
  assert.deepStrictEqual(actual, expected, `${name}: data-only fixture differs from source ${source}`);
  console.log(`${name}: data-only fixture equals source ${source}`);
}
