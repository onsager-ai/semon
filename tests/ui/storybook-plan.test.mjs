import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { makeCapturePlan } from './storybook-plan.mjs';
import { referenceErrors, referenceComparisonErrors, captureIdentityDigest } from './storybook-references.mjs';
import { assertionErrors, verifyAssertions } from './storybook-assertions.mjs';

const read = (file) => JSON.parse(fs.readFileSync(new URL(file, import.meta.url), 'utf8'));
const contract = read('./storybook-contract.json');
const inventory = read('./storybook-state-bindings.json');
const stories = [...new Set([
  ...contract.productionSurfaces.flatMap((item) => item.storyIds),
  ...Object.values(inventory.targets).flatMap((item) => item.storyId ? [item.storyId] : []),
  ...contract.interactionCaptures.map((item) => item.storyId),
])];
const plan = () => makeCapturePlan(structuredClone(contract), structuredClone(inventory), stories);

test('full matrix includes distinct collapsed tools, compact sizes and executable disclosures', () => {
  const cases = plan();
  for (const width of [334, 390, 696, 820, 1280]) for (const scheme of ['light', 'dark']) {
    const row = cases.find((item) => item.key === `semon-production-owners--compact-reader|${width}|${scheme}|related-session-expanded`);
    assert.ok(row?.actions.length && row.assertions.length);
  }
  const tool = cases.find((item) => item.key === 'semon-production-owners--collapsed-tool|1280|light');
  assert.ok(tool.assertions.some((item) => item.selector.includes('aria-expanded="false"')));
  assert.ok(tool.assertions.some((item) => item.kind === 'absent' && item.selector.endsWith('.out')));
});

test('singular action and misplaced waitMs cannot pass validation', () => {
  for (const field of ['action', 'waitMs']) {
    const changed = structuredClone(contract);
    changed.interactionCaptures[0][field] = field === 'action' ? changed.interactionCaptures[0].actions[0] : 650;
    assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), /singular action and top-level waitMs/);
  }
  const changed = structuredClone(contract);
  delete changed.interactionCaptures[0].actions;
  assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), /executable actions/);
});

test('account actions resolve to the actual phone drawer and desktop root', () => {
  const cases = plan().filter((item) => item.interactionId === 'gallery-account-menu-open');
  assert.ok(cases.length);
  for (const row of cases) {
    if (row.width === 390) {
      assert.deepEqual(row.actions.map((item) => item.selector), ['#lead-btn', '#account-drawer .account-trigger']);
      assert.ok(row.actions.every((item) => item.kind === 'tap'));
    } else assert.ok(row.actions[0].selector.includes('#topbar .account-widget-desktop'));
  }
  const changed = structuredClone(contract);
  const account = changed.interactionCaptures.find((item) => item.id === 'gallery-account-menu-open');
  delete account.widthActions['390'];
  assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), /executable actions/);
});

test('modal teardown uses only its scoped owner hook and remounts an operable sheet', () => {
  const item = plan().find((row) => row.interactionId === 'gallery-overlay-lifecycle');
  assert.equal(item.actions[1].kind, 'owner-unmount');
  assert.ok(item.actions[0].assertionsAfter.some((row) => row.selector === 'dialog[open]'));
  assert.ok(item.postCaptureActions.some((row) => row.selector === '#sheet-trigger'));
  const changed = structuredClone(contract);
  changed.interactionCaptures.find((row) => row.id === item.interactionId).storyId = stories[0];
  assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), /restricted to its test-only lifecycle owner/);
});

test('unknown stories, themes, widths and action kinds fail closed', () => {
  for (const [field, value, pattern] of [
    ['storyId', 'missing--owner', /unknown story/],
    ['schemes', ['sepia'], /unsupported theme/],
    ['widths', [999], /unsupported width/],
    ['actions', [{kind: 'evaluate', selector: 'body'}], /unsupported action/],
  ]) {
    const changed = structuredClone(contract);
    changed.interactionCaptures[0][field] = value;
    assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), pattern);
  }
});

test('references remain pending until exact provenance and a real review are recorded', () => {
  const map = read('./storybook-reference-map.json'), cases = plan();
  assert.deepEqual(referenceErrors({ ...map, approved: {} }, cases), []);
  const unknown = structuredClone(map);
  unknown.approved['missing|390|light'] = { decision: 'accepted' };
  assert.match(referenceErrors(unknown, cases).join('\n'), /unknown approved capture/);
  const incomplete = structuredClone(map);
  incomplete.approved[cases[0].key] = { decision: 'accepted', path: 'baselines/storybook/draft.png' };
  assert.match(referenceErrors(incomplete, cases).join('\n'), /invalid\/unreviewed reference provenance/);
  const draft = structuredClone(map);
  draft.approved[cases[0].key] = { decision: 'draft', path: 'baselines/storybook/draft.png' };
  assert.match(referenceErrors(draft, cases).join('\n'), /invalid\/unreviewed reference provenance/);
});

test('reviewed historical references compare across source revisions without rewriting provenance', () => {
  const item = plan()[0], bytes = Buffer.from('synthetic reviewed reference bytes');
  const digest = (value) => createHash('sha256').update(value).digest('hex');
  const historicalRevision = 'a'.repeat(40), currentRevision = 'b'.repeat(40);
  const historicalSource = digest('reviewed production sources'), currentSource = digest('edited production sources');
  const currentCandidate = { sourceDigest: currentSource, sourceRevision: currentRevision };
  const record = {
    decision: 'accepted', path: 'baselines/storybook/reviewed.png', sha256: digest(bytes),
    storyId: item.storyId, width: item.width, scheme: item.scheme,
    threshold: 0.1, maxRatio: 0.0001,
    sourceDigest: historicalSource, captureDigest: captureIdentityDigest(item),
    reviewedRevision: historicalRevision, reviewedBy: 'Synthetic reviewer',
    reviewedAt: '2026-10-19T07:00:00.000Z', rationale: 'Reviewed exact capture and geometry',
    provenance: { source: historicalRevision, state: item.expectedState, geometry: `${item.width}px ${item.scheme}` },
  };
  const before = structuredClone(record);
  assert.notEqual(currentSource, historicalSource);
  assert.notEqual(currentRevision, historicalRevision);
  assert.deepEqual(referenceComparisonErrors(record, item, bytes, currentCandidate), []);
  assert.deepEqual(record, before, 'validation must not rewrite historical metadata');
  for (const field of ['sourceDigest', 'reviewedRevision', 'reviewedBy', 'reviewedAt', 'provenance']) {
    const invalid = structuredClone(record);
    delete invalid[field];
    assert.ok(referenceComparisonErrors(invalid, item, bytes, currentCandidate).length, field);
  }
  assert.ok(referenceComparisonErrors({ ...record, sourceDigest: 'not-a-digest' }, item, bytes, currentCandidate).length);
  assert.ok(referenceComparisonErrors({ ...record, reviewedRevision: currentRevision.slice(1) }, item, bytes, currentCandidate).length);
  assert.match(referenceComparisonErrors(record, { ...item, expectedState: 'different state' }, bytes, currentCandidate).join('\n'), /capture identity changed/);
  assert.ok(referenceComparisonErrors(record, { ...item, width: 820 }, bytes, currentCandidate).length);
  assert.ok(referenceComparisonErrors(record, { ...item, scheme: 'dark' }, bytes, currentCandidate).length);
  assert.ok(referenceComparisonErrors({ ...record, maxRatio: 0.5 }, item, bytes, currentCandidate).length);
  assert.match(referenceComparisonErrors(record, item, Buffer.from('changed reference bytes'), currentCandidate).join('\n'), /reference digest changed/i);
});

// A deliberately strict synthetic collection: waitFor/evaluate on the collection
// itself throws, like Playwright. Only explicit enumeration can inspect its rows.
function syntheticPage(rows) {
  const inspected = [];
  const collection = (matches) => ({
    count: async () => matches.length,
    filter: ({ hasText }) => collection(matches.filter((row) => row.text.includes(hasText))),
    waitFor() { throw new Error('strict collection wait is forbidden'); },
    nth(index) {
      const row = matches[index];
      return {
        async waitFor({ state }) {
          inspected.push(row.attributes['data-id']);
          if (row.visible !== (state === 'visible')) throw new Error('row visibility differs');
        },
        getAttribute: async (name) => row.attributes[name] ?? null,
        innerText: async () => row.text,
        textContent: async () => row.text,
      };
    },
  });
  return { locator: () => collection(rows), inspected };
}

test('all six expected rows are checked without a strict collection wait or arbitrary first match', async () => {
  const rows = Array.from({ length: 6 }, (_, index) => ({ attributes: { 'data-id': `session-${index}` }, text: `Recorded session ${index}`, visible: true }));
  const assertion = { kind: 'visible', selector: '.session-list [data-session-row]', count: 6,
    rows: rows.map(({ attributes, text }) => ({ attributes, text })) };
  assert.deepEqual(assertionErrors(assertion), []);
  const page = syntheticPage(rows);
  await verifyAssertions(page, [assertion], { timeoutMs: 0 });
  assert.deepEqual(page.inspected, rows.map((row) => row.attributes['data-id']));
  await assert.rejects(verifyAssertions(syntheticPage(rows.slice(1)), [assertion], { timeoutMs: 0 }), /Expected 6.*got 5/);
  await assert.rejects(verifyAssertions(syntheticPage([...rows, rows[0]]), [assertion], { timeoutMs: 0 }), /Expected 6.*got 7/);
  await assert.rejects(verifyAssertions(syntheticPage(rows.map((row, index) => index === 5 ? { ...row, visible: false } : row)), [assertion], { timeoutMs: 0 }), /visibility differs/);
  await assert.rejects(verifyAssertions(syntheticPage(rows.map((row, index) => index === 5 ? { ...row, attributes: { 'data-id': 'wrong-row' } } : row)), [assertion], { timeoutMs: 0 }), /expected data-id=session-5/);
  await assert.rejects(verifyAssertions(syntheticPage(rows.map((row, index) => index === 5 ? { ...row, text: 'Wrong recorded session' } : row)), [assertion], { timeoutMs: 0 }), /expected text Recorded session 5/);
});

test('singular, hidden and absent assertions have explicit cardinality semantics', async () => {
  const rows = [{ attributes: { 'data-id': 'one' }, text: 'Expected status', visible: true }, { attributes: { 'data-id': 'two' }, text: 'Other status', visible: true }];
  await verifyAssertions(syntheticPage(rows), [{ kind: 'visible', selector: '.status', text: 'Expected status' }], { timeoutMs: 0 });
  await assert.rejects(verifyAssertions(syntheticPage(rows), [{ kind: 'visible', selector: '.status' }], { timeoutMs: 0 }), /Expected 1.*got 2/);
  await verifyAssertions(syntheticPage([]), [{ kind: 'absent', selector: 'dialog' }], { timeoutMs: 0 });
  await assert.rejects(verifyAssertions(syntheticPage(rows), [{ kind: 'absent', selector: 'dialog' }], { timeoutMs: 0 }), /Expected 0.*got 2/);
  await verifyAssertions(syntheticPage([{ ...rows[0], visible: false }]), [{ kind: 'hidden', selector: '.details' }], { timeoutMs: 0 });
  await assert.rejects(verifyAssertions(syntheticPage([]), [{ kind: 'hidden', selector: '.details' }], { timeoutMs: 0 }), /Expected 1.*got 0/);
  assert.ok(assertionErrors({ kind: 'visible', selector: '.row', count: 6 }).length);
  assert.ok(assertionErrors({ kind: 'visible', selector: '.row', count: -1 }).length);
  assert.ok(assertionErrors({ kind: 'visible', selector: '.row', count: 2, rows: [{ attributes: { id: 'same' }, text: 'one' }, { attributes: { id: 'same' }, text: 'two' }] }).length);
  const changed = structuredClone(inventory);
  changed.targets.catalog.assertions.push({ kind: 'visible', selector: '.row', count: 6 });
  assert.throws(() => makeCapturePlan(structuredClone(contract), changed, stories), /Plural assertions require expected row identities/);
});
