import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import { createHash } from 'node:crypto';
import { makeCapturePlan } from './storybook-plan.mjs';
import {
  referenceErrors,
  referenceComparisonErrors,
  captureIdentityDigest,
} from './storybook-references.mjs';
import { assertionErrors, verifyAssertions } from './storybook-assertions.mjs';
import { legacyCapturePlan, routeStoryIds, successorErrors } from './storybook-legacy.mjs';
import { EXPECTED, expectedPixelCoverage } from './preservation-contract.mjs';
import { nativeArtifactPaths } from './storybook-native-artifacts.mjs';
import { witnessDifferences } from './storybook-presentation-witness.mjs';

const read = (file) => JSON.parse(fs.readFileSync(new URL(file, import.meta.url), 'utf8'));
const contract = read('./storybook-contract.json');
const inventory = read('./storybook-state-bindings.json');
const stories = [
  ...new Set([
    ...routeStoryIds,
    'semon-legacy-gallery--gallery',
    ...contract.productionSurfaces.flatMap((item) => item.storyIds),
    ...Object.values(inventory.targets).flatMap((item) => (item.storyId ? [item.storyId] : [])),
    ...contract.interactionCaptures.map((item) => item.storyId),
  ]),
];
const plan = () => makeCapturePlan(structuredClone(contract), structuredClone(inventory), stories);

test('legacy capture identities retain native tail, region and preservation reset policies', () => {
  const cases = legacyCapturePlan();
  const get = (key) => cases.find((item) => item.legacyKey === key);
  assert.equal(get('pixels:main:frame:phone-light|session-harbor').capture.scroll, 'document-tail');
  assert.equal(get('pixels:main:frame:desktop-light|session-harbor').capture.scroll, 'main-tail');
  assert.equal(get('pixels:main:frame:phone-light|home').capture.scroll, 'top');
  assert.equal(
    get('pixels:main:region:phone-light|session-harbor|transcript').capture.scroll,
    'region-top',
  );
  assert.equal(
    get('preservation:required:frame:390-light-session-harbor').capture.scroll,
    'preservation-reset',
  );
  const selected = get('pixels:main:frame:phone-light|session-harbor');
  assert.notEqual(
    captureIdentityDigest(selected),
    captureIdentityDigest({ ...selected, capture: { ...selected.capture, scroll: 'top' } }),
  );
});

test('native artifact binding keeps all scopes and region keys separate', () => {
  const item = (legacyKey) => ({ legacyKey });
  assert.equal(
    nativeArtifactPaths('/native', item('pixels:two-machines:frame:phone-light|machine-desktop'))
      .image,
    '/native/viewer-ui-transport/two-machines/pixels/phone-light_machine-desktop-candidate.png',
  );
  assert.equal(
    nativeArtifactPaths(
      '/native',
      item('pixels:pushed:region:phone-light|session-harbor|transcript'),
    ).witness,
    '/native/viewer-ui-transport/pushed/pixels/phone-light_session-harbor_transcript-candidate-presentation.json',
  );
  assert.equal(
    nativeArtifactPaths(
      '/native',
      item('preservation:required:frame:390-light-harbor-thinking-open'),
    ).image,
    '/native/viewer-ui-visual/preservation/390-light-harbor-thinking-open-candidate.png',
  );
});

test('native presentation comparison never normalizes source, text, order, disclosure or geometry', () => {
  const native = {
    text: 'Sentinel',
    source: '["laptop","principal"]',
    open: true,
    box: [0, 44, 390, 60],
  };
  assert.deepEqual(witnessDifferences(native, structuredClone(native)), []);
  for (const changed of [
    { text: 'Principal' },
    { source: '["desktop","principal"]' },
    { open: false },
    { box: [0, 45, 390, 60] },
    { extra: 'unknown' },
  ])
    assert.ok(witnessDifferences(native, { ...native, ...changed }).length > 0);
  assert.ok(witnessDifferences([native, { text: 'next' }], [{ text: 'next' }, native]).length > 0);
});

function expectedLegacyKeys() {
  const keys = [];
  for (const scope of ['main', 'two-machines', 'pushed']) {
    const coverage = expectedPixelCoverage(scope);
    for (const kind of ['frame', 'region'])
      for (const key of coverage[kind === 'frame' ? 'frames' : 'regions'])
        keys.push(`pixels:${scope}:${kind}:${key}`);
  }
  keys.push(...EXPECTED.map((key) => 'preservation:required:frame:' + key));
  const gallery = read('./gallery-visual.json');
  for (const state of gallery.states)
    for (const width of gallery.widths)
      for (const scheme of gallery.schemes)
        keys.push(`gallery:required:frame:${state}-${width}-${scheme}`);
  return keys;
}

test('every exact legacy key has one discoverable executable successor, with no aliases', () => {
  const cases = plan(),
    expected = expectedLegacyKeys();
  assert.deepEqual(successorErrors(expected, cases), []);
  assert.equal(cases.filter((row) => row.legacyKey).length, expected.length);
  assert.equal(cases.filter((row) => row.legacyKey?.startsWith('preservation:')).length, 68);
  for (const item of cases.filter((row) => row.legacyKey)) {
    assert.ok(stories.includes(item.storyId));
    assert.equal(item.capture.masks.length, 0);
    assert.equal(item.pixelPolicy.maxRatio, item.legacyKey.startsWith('gallery:') ? 0.001 : 0.0001);
    assert.equal(item.viewport.deviceScaleFactor, item.width === 390 ? 2 : 1);
    if (item.legacyKey.startsWith('preservation:')) {
      assert.equal(item.capture.fullPage, false);
      assert.equal(item.capture.animations, 'allow');
      assert.equal(item.capture.caret, 'hide');
      assert.equal(item.capture.stablePaint, true);
    }
  }
});

test('exact successor audit rejects missing, unknown, duplicate and incomplete bindings', () => {
  const expected = expectedLegacyKeys(),
    cases = legacyCapturePlan();
  assert.match(successorErrors(expected, cases.slice(1)).join('\n'), /Missing Storybook successor/);
  assert.match(
    successorErrors(expected, [...cases, cases[0]]).join('\n'),
    /Duplicate legacy successor/,
  );
  assert.match(
    successorErrors(expected, [
      ...cases,
      { ...cases[0], key: 'orphan', legacyKey: 'pixels:main:frame:unknown' },
    ]).join('\n'),
    /Unknown legacy successor/,
  );
  assert.match(
    successorErrors(expected, [{ ...cases[0], fixture: null }, ...cases.slice(1)]).join('\n'),
    /Incomplete exact successor identity/,
  );
  for (const change of [
    { storyId: 'semon-production-owners--home' },
    { viewport: { ...cases[0].viewport, height: 900 } },
    { capture: { ...cases[0].capture, selector: '#topbar' } },
    { fixture: { ...cases[0].fixture, scope: 'pushed' } },
  ])
    assert.match(
      successorErrors(expected, [{ ...cases[0], ...change }, ...cases.slice(1)]).join('\n'),
      /Changed explicit legacy successor binding/,
    );
  assert.throws(
    () =>
      makeCapturePlan(
        contract,
        inventory,
        stories.filter((id) => id !== 'semon-legacy-routes--machine-desktop'),
      ),
    /unknown legacy story/,
  );
});

test('legacy disclosure, filter focus/scroll and compact account states use real scoped controls', () => {
  const cases = legacyCapturePlan();
  const output = cases.find(
    (row) => row.legacyKey === 'preservation:required:frame:820-dark-harbor-tool-output-open',
  );
  assert.deepEqual(
    output.actions.map((action) => action.selector),
    ['.think-disclosure >> nth=0', '.tsum >> nth=0', '.tgroup .steps .step > button >> nth=0'],
  );
  const filter = cases.find(
    (row) => row.legacyKey === 'preservation:required:frame:390-light-sessions-filter-open',
  );
  assert.deepEqual(
    filter.actions.map((action) => action.kind),
    ['click', 'focus', 'scroll-top'],
  );
  for (const scheme of ['light', 'dark']) {
    const account = cases.find(
      (row) => row.legacyKey === `gallery:required:frame:account-390-${scheme}`,
    );
    assert.deepEqual(
      account.actions.slice(-2).map((action) => action.selector),
      ['#lead-btn', '#account-drawer .account-trigger'],
    );
    assert.deepEqual(
      account.actions.slice(0, 4).map((action) => action.key ?? action.kind),
      ['focus', 'ArrowDown', 'ArrowDown', 'Enter'],
    );
    assert.ok(account.assertions.some((assertion) => assertion.text === 'Oldest first'));
  }
  assert.deepEqual(
    assertionErrors({ kind: 'visible', selector: '.filters-sheet .vb', scrollTop: 0 }),
    [],
  );
  assert.ok(
    assertionErrors({ kind: 'visible', selector: '.filters-sheet .vb', scrollTop: -1 }).length,
  );
});

test('gallery provenance permits only its original case policy and retains strict native policy', () => {
  const gallery = legacyCapturePlan().find((row) => row.legacyKey.startsWith('gallery:'));
  // A draft may never become valid simply because its policy matches gallery.
  const map = {
    $schema: './storybook-reference-map.schema.json',
    version: 2,
    pending: 'review required',
    approved: { [gallery.key]: { decision: 'draft', threshold: 0.1, maxRatio: 0.001 } },
  };
  assert.ok(referenceErrors(map, [gallery]).length);
  const digest = (value) => createHash('sha256').update(value).digest('hex');
  const bytes = Buffer.from('synthetic reviewed gallery bytes');
  const record = {
    decision: 'accepted',
    path: 'baselines/storybook/reviewed-gallery.png',
    sha256: digest(bytes),
    storyId: gallery.storyId,
    width: gallery.width,
    scheme: gallery.scheme,
    threshold: 0.1,
    maxRatio: 0.001,
    sourceDigest: digest('historical gallery source'),
    captureDigest: captureIdentityDigest(gallery),
    reviewedRevision: 'a'.repeat(40),
    reviewedBy: 'Synthetic reviewer',
    reviewedAt: '2026-10-19T07:00:00.000Z',
    rationale: 'Exact original gallery policy',
    provenance: {
      source: 'original gallery owner',
      state: gallery.expectedState,
      geometry: 'original gallery frame',
    },
  };
  assert.deepEqual(
    referenceComparisonErrors(record, gallery, bytes, {
      sourceDigest: digest('edited candidate'),
      sourceRevision: 'b'.repeat(40),
    }),
    [],
  );
  assert.ok(
    referenceErrors({ ...map, approved: { [gallery.key]: { ...record, maxRatio: 0.0001 } } }, [
      gallery,
    ]).length,
  );
  const strict = legacyCapturePlan().find((row) => row.legacyKey.startsWith('preservation:'));
  assert.ok(
    referenceErrors(
      {
        ...map,
        approved: {
          [strict.key]: {
            ...record,
            storyId: strict.storyId,
            width: strict.width,
            scheme: strict.scheme,
            captureDigest: captureIdentityDigest(strict),
          },
        },
      },
      [strict],
    ).length,
  );
});

test('full matrix includes distinct collapsed tools, compact sizes and executable disclosures', () => {
  const cases = plan();
  for (const width of [334, 390, 696, 820, 1280])
    for (const scheme of ['light', 'dark']) {
      const row = cases.find(
        (item) =>
          item.key ===
          `semon-production-owners--compact-reader|${width}|${scheme}|related-session-expanded`,
      );
      assert.ok(row?.actions.length && row.assertions.length);
    }
  const tool = cases.find(
    (item) => item.key === 'semon-production-owners--collapsed-tool|1280|light',
  );
  assert.ok(tool.assertions.some((item) => item.selector.includes('aria-expanded="false"')));
  assert.ok(
    tool.assertions.some((item) => item.kind === 'absent' && item.selector.endsWith('.out')),
  );
});

test('singular action and misplaced waitMs cannot pass validation', () => {
  for (const field of ['action', 'waitMs']) {
    const changed = structuredClone(contract);
    changed.interactionCaptures[0][field] =
      field === 'action' ? changed.interactionCaptures[0].actions[0] : 650;
    assert.throws(
      () => makeCapturePlan(changed, structuredClone(inventory), stories),
      /singular action and top-level waitMs/,
    );
  }
  const changed = structuredClone(contract);
  delete changed.interactionCaptures[0].actions;
  assert.throws(
    () => makeCapturePlan(changed, structuredClone(inventory), stories),
    /executable actions/,
  );
});

test('account actions resolve to the actual phone drawer and desktop root', () => {
  const cases = plan().filter((item) => item.interactionId === 'gallery-account-menu-open');
  assert.ok(cases.length);
  for (const row of cases) {
    if (row.width === 390) {
      assert.deepEqual(
        row.actions.map((item) => item.selector),
        ['#lead-btn', '#account-drawer .account-trigger'],
      );
      assert.ok(row.actions.every((item) => item.kind === 'tap'));
    } else assert.ok(row.actions[0].selector.includes('#topbar .account-widget-desktop'));
  }
  const changed = structuredClone(contract);
  const account = changed.interactionCaptures.find(
    (item) => item.id === 'gallery-account-menu-open',
  );
  delete account.widthActions['390'];
  assert.throws(
    () => makeCapturePlan(changed, structuredClone(inventory), stories),
    /executable actions/,
  );
});

test('modal teardown uses only its scoped owner hook and remounts an operable sheet', () => {
  const item = plan().find((row) => row.interactionId === 'gallery-overlay-lifecycle');
  assert.equal(item.actions[1].kind, 'owner-unmount');
  assert.ok(item.actions[0].assertionsAfter.some((row) => row.selector === 'dialog[open]'));
  assert.ok(item.postCaptureActions.some((row) => row.selector === '#sheet-trigger'));
  const changed = structuredClone(contract);
  changed.interactionCaptures.find((row) => row.id === item.interactionId).storyId = stories[0];
  assert.throws(
    () => makeCapturePlan(changed, structuredClone(inventory), stories),
    /restricted to its test-only lifecycle owner/,
  );
});

test('unknown stories, themes, widths and action kinds fail closed', () => {
  for (const [field, value, pattern] of [
    ['storyId', 'missing--owner', /unknown story/],
    ['schemes', ['sepia'], /unsupported theme/],
    ['widths', [999], /unsupported width/],
    ['actions', [{ kind: 'evaluate', selector: 'body' }], /unsupported action/],
  ]) {
    const changed = structuredClone(contract);
    changed.interactionCaptures[0][field] = value;
    assert.throws(() => makeCapturePlan(changed, structuredClone(inventory), stories), pattern);
  }
});

test('references remain pending until exact provenance and a real review are recorded', () => {
  const map = read('./storybook-reference-map.json'),
    cases = plan();
  assert.deepEqual(referenceErrors({ ...map, approved: {} }, cases), []);
  const unknown = structuredClone(map);
  unknown.approved['missing|390|light'] = { decision: 'accepted' };
  assert.match(referenceErrors(unknown, cases).join('\n'), /unknown approved capture/);
  const incomplete = structuredClone(map);
  incomplete.approved[cases[0].key] = {
    decision: 'accepted',
    path: 'baselines/storybook/draft.png',
  };
  assert.match(
    referenceErrors(incomplete, cases).join('\n'),
    /invalid\/unreviewed reference provenance/,
  );
  const draft = structuredClone(map);
  draft.approved[cases[0].key] = { decision: 'draft', path: 'baselines/storybook/draft.png' };
  assert.match(
    referenceErrors(draft, cases).join('\n'),
    /invalid\/unreviewed reference provenance/,
  );
});

test('legacy capture identities stay stable across candidate revisions while references retain provenance', () => {
  const bytes = Buffer.from('synthetic reviewed reference bytes');
  const digest = (value) => createHash('sha256').update(value).digest('hex');
  const historicalRevision = 'a'.repeat(40),
    currentRevision = 'b'.repeat(40);
  const previousRevision = process.env.GITHUB_SHA;
  let historicalPlan, currentPlan;
  try {
    process.env.GITHUB_SHA = historicalRevision;
    historicalPlan = legacyCapturePlan();
    process.env.GITHUB_SHA = currentRevision;
    currentPlan = legacyCapturePlan();
  } finally {
    if (previousRevision === undefined) delete process.env.GITHUB_SHA;
    else process.env.GITHUB_SHA = previousRevision;
  }
  const identities = (items) => items.map((item) => [item.key, captureIdentityDigest(item)]);
  assert.deepEqual(identities(currentPlan), identities(historicalPlan));
  const key = 'pixels:main:frame:desktop-light|session-harbor';
  const item = historicalPlan.find((candidate) => candidate.legacyKey === key);
  const currentItem = currentPlan.find((candidate) => candidate.legacyKey === key);
  assert.ok(item, 'a concrete selected-session route item is present');
  assert.ok(
    currentItem,
    'the same exact selected-session route item is present at the candidate revision',
  );
  const historicalSource = digest('reviewed production sources'),
    currentSource = digest('edited production sources');
  const currentCandidate = { sourceDigest: currentSource, sourceRevision: currentRevision };
  const record = {
    decision: 'accepted',
    path: 'baselines/storybook/reviewed.png',
    sha256: digest(bytes),
    storyId: item.storyId,
    width: item.width,
    scheme: item.scheme,
    threshold: 0.1,
    maxRatio: 0.0001,
    sourceDigest: historicalSource,
    captureDigest: captureIdentityDigest(item),
    reviewedRevision: historicalRevision,
    reviewedBy: 'Synthetic reviewer',
    reviewedAt: '2026-10-19T07:00:00.000Z',
    rationale: 'Reviewed exact capture and geometry',
    provenance: {
      source: historicalRevision,
      state: item.expectedState,
      geometry: `${item.width}px ${item.scheme}`,
    },
  };
  const before = structuredClone(record);
  assert.notEqual(currentSource, historicalSource);
  assert.notEqual(currentRevision, historicalRevision);
  assert.deepEqual(referenceComparisonErrors(record, currentItem, bytes, currentCandidate), []);
  assert.deepEqual(record, before, 'validation must not rewrite historical metadata');
  for (const field of [
    'sourceDigest',
    'reviewedRevision',
    'reviewedBy',
    'reviewedAt',
    'provenance',
  ]) {
    const invalid = structuredClone(record);
    delete invalid[field];
    assert.ok(
      referenceComparisonErrors(invalid, currentItem, bytes, currentCandidate).length,
      field,
    );
  }
  assert.ok(
    referenceComparisonErrors(
      { ...record, sourceDigest: 'not-a-digest' },
      currentItem,
      bytes,
      currentCandidate,
    ).length,
  );
  assert.ok(
    referenceComparisonErrors(
      { ...record, reviewedRevision: currentRevision.slice(1) },
      currentItem,
      bytes,
      currentCandidate,
    ).length,
  );
  for (const changed of [
    { expectedState: 'different state' },
    { capture: { ...currentItem.capture, scroll: 'top' } },
    { viewport: { ...currentItem.viewport, height: currentItem.viewport.height + 1 } },
  ]) {
    assert.match(
      referenceComparisonErrors(
        record,
        { ...currentItem, ...changed },
        bytes,
        currentCandidate,
      ).join('\n'),
      /capture identity changed/,
    );
  }
  assert.ok(
    referenceComparisonErrors(record, { ...currentItem, width: 820 }, bytes, currentCandidate)
      .length,
  );
  assert.ok(
    referenceComparisonErrors(record, { ...currentItem, scheme: 'dark' }, bytes, currentCandidate)
      .length,
  );
  assert.ok(
    referenceComparisonErrors({ ...record, maxRatio: 0.5 }, currentItem, bytes, currentCandidate)
      .length,
  );
  assert.match(
    referenceComparisonErrors(
      record,
      currentItem,
      Buffer.from('changed reference bytes'),
      currentCandidate,
    ).join('\n'),
    /reference digest changed/i,
  );
});

// A deliberately strict synthetic collection: waitFor/evaluate on the collection
// itself throws, like Playwright. Only explicit enumeration can inspect its rows.
function syntheticPage(rows) {
  const inspected = [];
  const collection = (matches) => ({
    count: async () => matches.length,
    filter: ({ hasText }) => collection(matches.filter((row) => row.text.includes(hasText))),
    waitFor() {
      throw new Error('strict collection wait is forbidden');
    },
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
  const rows = Array.from({ length: 6 }, (_, index) => ({
    attributes: { 'data-id': `session-${index}` },
    text: `Recorded session ${index}`,
    visible: true,
  }));
  const assertion = {
    kind: 'visible',
    selector: '.session-list [data-session-row]',
    count: 6,
    rows: rows.map(({ attributes, text }) => ({ attributes, text })),
  };
  assert.deepEqual(assertionErrors(assertion), []);
  const page = syntheticPage(rows);
  await verifyAssertions(page, [assertion], { timeoutMs: 0 });
  assert.deepEqual(
    page.inspected,
    rows.map((row) => row.attributes['data-id']),
  );
  await assert.rejects(
    verifyAssertions(syntheticPage(rows.slice(1)), [assertion], { timeoutMs: 0 }),
    /Expected 6.*got 5/,
  );
  await assert.rejects(
    verifyAssertions(syntheticPage([...rows, rows[0]]), [assertion], { timeoutMs: 0 }),
    /Expected 6.*got 7/,
  );
  await assert.rejects(
    verifyAssertions(
      syntheticPage(rows.map((row, index) => (index === 5 ? { ...row, visible: false } : row))),
      [assertion],
      { timeoutMs: 0 },
    ),
    /visibility differs/,
  );
  await assert.rejects(
    verifyAssertions(
      syntheticPage(
        rows.map((row, index) =>
          index === 5 ? { ...row, attributes: { 'data-id': 'wrong-row' } } : row,
        ),
      ),
      [assertion],
      { timeoutMs: 0 },
    ),
    /expected data-id=session-5/,
  );
  await assert.rejects(
    verifyAssertions(
      syntheticPage(
        rows.map((row, index) => (index === 5 ? { ...row, text: 'Wrong recorded session' } : row)),
      ),
      [assertion],
      { timeoutMs: 0 },
    ),
    /expected text Recorded session 5/,
  );
});

test('singular, hidden and absent assertions have explicit cardinality semantics', async () => {
  const rows = [
    { attributes: { 'data-id': 'one' }, text: 'Expected status', visible: true },
    { attributes: { 'data-id': 'two' }, text: 'Other status', visible: true },
  ];
  await verifyAssertions(
    syntheticPage(rows),
    [{ kind: 'visible', selector: '.status', text: 'Expected status' }],
    { timeoutMs: 0 },
  );
  await assert.rejects(
    verifyAssertions(syntheticPage(rows), [{ kind: 'visible', selector: '.status' }], {
      timeoutMs: 0,
    }),
    /Expected 1.*got 2/,
  );
  await verifyAssertions(syntheticPage([]), [{ kind: 'absent', selector: 'dialog' }], {
    timeoutMs: 0,
  });
  await assert.rejects(
    verifyAssertions(syntheticPage(rows), [{ kind: 'absent', selector: 'dialog' }], {
      timeoutMs: 0,
    }),
    /Expected 0.*got 2/,
  );
  await verifyAssertions(
    syntheticPage([{ ...rows[0], visible: false }]),
    [{ kind: 'hidden', selector: '.details' }],
    { timeoutMs: 0 },
  );
  await assert.rejects(
    verifyAssertions(syntheticPage([]), [{ kind: 'hidden', selector: '.details' }], {
      timeoutMs: 0,
    }),
    /Expected 1.*got 0/,
  );
  assert.ok(assertionErrors({ kind: 'visible', selector: '.row', count: 6 }).length);
  assert.ok(assertionErrors({ kind: 'visible', selector: '.row', count: -1 }).length);
  assert.ok(
    assertionErrors({
      kind: 'visible',
      selector: '.row',
      count: 2,
      rows: [
        { attributes: { id: 'same' }, text: 'one' },
        { attributes: { id: 'same' }, text: 'two' },
      ],
    }).length,
  );
  const changed = structuredClone(inventory);
  changed.targets.catalog.assertions.push({ kind: 'visible', selector: '.row', count: 6 });
  assert.throws(
    () => makeCapturePlan(structuredClone(contract), changed, stories),
    /Plural assertions require expected row identities/,
  );
});
