import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const args = process.argv.slice(2);
if (args.length && (args.length !== 1 || args[0] !== '--render-only'))
  throw new Error('Usage: storybook-report.mjs [--render-only]');
const here = path.dirname(fileURLToPath(import.meta.url));
const out = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook');
const contract = JSON.parse(fs.readFileSync(path.join(out, 'contract-report.json'), 'utf8'));
const capture = JSON.parse(
  fs.readFileSync(path.join(out, 'candidates/capture-report.json'), 'utf8'),
);
const expected = new Set(contract.executableStoryCaptures.map((item) => item.key));
const failures = [...contract.errors];
const keys = (items) => [...items].sort().join('\n');
if (!expected.size || keys(expected) !== keys(capture.expectedKeys ?? []))
  failures.push('capture plan differs from the validated full inventory');
const seen = new Set();
for (const row of capture.results ?? []) {
  if (!expected.has(row.key) || seen.has(row.key))
    failures.push(`unknown/duplicate capture ${row.key}`);
  seen.add(row.key);
  if (!row.rendered || !row.candidate || row.error)
    failures.push(`${row.key}: ${row.error ?? 'missing rendered candidate'}`);
}
for (const key of expected) if (!seen.has(key)) failures.push(`missing capture ${key}`);
if (capture.approvedReferencesWritten !== false || capture.candidateOnly !== true)
  failures.push('capture attempted to write reference approvals');
if (process.env.GITHUB_SHA && capture.sourceRevision !== process.env.GITHUB_SHA)
  failures.push('capture came from a different source revision');
if (!args.includes('--render-only')) {
  failures.push(...(capture.failures ?? ['missing failure report']));
  if (!Array.isArray(capture.pending) || capture.pending.length)
    failures.push(
      `${capture.pending?.length ?? 'unknown'} Storybook references pending separate image inspection and Git approval`,
    );
  for (const row of capture.results ?? [])
    if (row.reference !== 'accepted' || row.comparisonError)
      failures.push(`${row.key}: ${row.comparisonError ?? 'no accepted exact reference'}`);
}
if (failures.length) {
  console.error(
    `${args.includes('--render-only') ? 'Storybook render gate' : 'Storybook visual gate'}: ${failures.length} failures`,
  );
  for (const failure of failures.slice(0, 30)) console.error(`- ${failure}`);
  if (failures.length > 30)
    console.error('See the uploaded contract/capture reports for remaining exact keys.');
  process.exitCode = 1;
} else
  console.log(
    `${args.includes('--render-only') ? 'Storybook render gate' : 'Storybook visual gate'}: all ${expected.size} required states passed.`,
  );
