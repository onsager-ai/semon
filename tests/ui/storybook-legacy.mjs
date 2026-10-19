import fs from 'node:fs';

// Independent, authored successor inventory. This never imports the expected
// coverage manifests: the CI validator compares those against these bindings.
// Each expanded record owns exactly one legacy key and one executable capture.
// Fixture reads and plan expansion run in CI, never as a local audit.
const fixture = JSON.parse(fs.readFileSync(new URL('./fixture-data/overhaul.json', import.meta.url), 'utf8'));
export const routeStories = [
  ['analytics', 'analytics', 'Analytics', []],
  ['drawer', 'drawer', 'Home', []],
  ['home', 'home', 'Home', ['topbar']],
  ['machine-laptop', 'machine-laptop', 'laptop', ['machine-content', 'topbar']],
  ['machines', 'machines', 'Machines', ['topbar']],
  ['session-a-docs', 'session-a-docs', 'Update the ingest docs', ['topbar', 'transcript']],
  ['session-a-index', 'session-a-index', 'feat/ingest-index', ['topbar', 'transcript']],
  ['session-a-parse', 'session-a-parse', 'feat/ingest-parser', ['topbar', 'transcript']],
  ['session-a-review', 'session-a-review', 'Review the parser diff', ['topbar', 'transcript']],
  ['session-a-review-grep', 'session-a-review-grep', 'Find parse_frame callers', ['topbar', 'transcript']],
  ['session-a-scan', 'session-a-scan', 'Survey ingest call sites', ['topbar', 'transcript']],
  ['session-a-store', 'session-a-store', 'feat/ingest-store', ['topbar', 'transcript']],
  ['session-advisor', 'session-advisor', 'Advisor', ['topbar', 'transcript']],
  ['session-atlas', 'session-atlas', 'atlas', ['topbar', 'transcript']],
  ['session-atlas-ingest', 'session-atlas-ingest', 'atlas ingest', ['topbar', 'menu', 'transcript']],
  ['session-deps', 'session-deps', 'deps audit', ['topbar', 'menu', 'transcript']],
  ['session-h-codex', 'session-h-codex', 'feat/offline-sync', ['topbar', 'transcript']],
  ['session-h-failed', 'session-h-failed', 'Check retry safety', ['topbar', 'transcript']],
  ['session-h-review', 'session-h-review', 'Review offline-sync diff', ['topbar', 'transcript']],
  ['session-h-review-codex', 'session-h-review-codex', 'Check flush ordering', ['topbar', 'transcript']],
  ['session-harbor', 'session-harbor', 'harbor', ['topbar', 'menu', 'transcript']],
  ['session-l-sub', 'session-l-sub', 'Find rounding call sites', ['topbar', 'transcript']],
  ['session-ledger', 'session-ledger', 'ledger', ['topbar', 'transcript']],
  ['session-principal', 'session-principal', 'Principal', ['topbar', 'transcript']],
  ['session-q-codex', 'session-q-codex', 'feat/pdf-export', ['topbar', 'transcript']],
  ['session-q-plan', 'session-q-plan', 'Draft the migration plan', ['topbar', 'transcript']],
  ['session-quill', 'session-quill', 'quill', ['topbar', 'transcript']],
  ['session-sentinel', 'session-sentinel', 'Sentinel', ['topbar', 'transcript']],
  ['sessions', 'sessions', 'Sessions', ['topbar']],
  ['trace-a-review-a-h-review', 'trace-a-review-a-h-review', 'Review the parser diff', []],
  ['trace-atlas-h12', 'trace-atlas-h12', 'atlas', []],
  ['trace-atlas-ingest-a-ask', 'trace-atlas-ingest-a-ask', 'atlas ingest', []],
  ['trace-h-review-h3', 'trace-h-review-h3', 'Review offline-sync diff', []],
  ['trace-harbor-h1', 'trace-harbor-h1', 'harbor', []],
  ['trace-ledger-h16', 'trace-ledger-h16', 'ledger', []],
  ['trace-principal-h8', 'trace-principal-h8', 'Principal', []],
  ['trace-quill-h4', 'trace-quill-h4', 'quill', []],
  ['trace-sentinel-sentinel_0', 'trace-sentinel-sentinel-0', 'Sentinel', []],
  ['machine-desktop', 'machine-desktop', 'desktop', ['machine-content', 'topbar']],
];
const story = (suffix) => 'semon-legacy-routes--' + suffix;
export const routeStoryIds = routeStories.map(([, suffix]) => story(suffix));
const scopes = [
  { scope: 'main', frames: [
    'analytics', 'drawer', 'home', 'machine-laptop', 'machines',
    'session-a-docs', 'session-a-index', 'session-a-parse', 'session-a-review', 'session-a-review-grep', 'session-a-scan', 'session-a-store',
    'session-advisor', 'session-atlas', 'session-atlas-ingest', 'session-deps', 'session-h-codex', 'session-h-failed', 'session-h-review', 'session-h-review-codex', 'session-harbor',
    'session-l-sub', 'session-ledger', 'session-principal', 'session-q-codex', 'session-q-plan', 'session-quill', 'session-sentinel', 'sessions',
    'trace-a-review-a-h-review', 'trace-atlas-h12', 'trace-atlas-ingest-a-ask', 'trace-h-review-h3', 'trace-harbor-h1', 'trace-ledger-h16', 'trace-principal-h8', 'trace-quill-h4', 'trace-sentinel-sentinel_0',
  ], schemes: ['phone-light', 'phone-dark', 'desktop-light'] },
  { scope: 'two-machines', frames: ['machines', 'machine-laptop', 'machine-desktop'], schemes: ['phone-light', 'phone-dark', 'desktop-light', 'desktop-dark'] },
  { scope: 'pushed', frames: ['home', 'sessions', 'analytics', 'session-harbor', 'session-deps'], schemes: ['phone-light', 'phone-dark', 'desktop-light'] },
];
const regions = {
  'machine-content': { selector: '#page', top: true },
  topbar: { selector: '#topbar', top: true },
  menu: { selector: 'dialog.panel', top: false, open: '#more-btn' },
  transcript: { selector: '#page section[aria-label="Transcript"]', top: true, viewportHeight: 'fit-transcript-and-dock-plus-80' },
};
const baseStates = [
  ['home', 'home'], ['sessions', 'sessions'], ['session-harbor', 'session-harbor'],
  ['harbor-thinking-open', 'session-harbor'], ['harbor-tools-open', 'session-harbor'], ['harbor-tool-output-open', 'session-harbor'],
  ['session-atlas-ingest', 'session-atlas-ingest'], ['session-deps', 'session-deps'], ['session-h-failed', 'session-h-failed'], ['analytics', 'analytics'], ['sessions-filter-open', 'sessions'],
];
const firstThinking = '.think-disclosure >> nth=0';
const firstTools = '.tsum >> nth=0';
const firstOutput = '.tgroup .steps .step > button >> nth=0';
function stateActions(state) {
  if (state === 'drawer') return [{ kind: 'click', selector: '#lead-btn' }];
  if (state === 'sessions-filter-open') return [
    { kind: 'click', selector: '.facet-btn' }, { kind: 'focus', selector: '.filters-sheet .vclose' }, { kind: 'scroll-top', selector: '.filters-sheet .vb' },
  ];
  const thinking = { kind: 'click', selector: firstThinking };
  const tools = { kind: 'click', selector: firstTools };
  if (state === 'harbor-thinking-open') return [thinking];
  if (state === 'harbor-tools-open') return [thinking, tools];
  if (state === 'harbor-tool-output-open') return [thinking, tools, { kind: 'click', selector: firstOutput }];
  return [];
}
function routeAssertions(frame, title, width) {
  const assertions = [{ kind: 'visible', selector: `.app[data-frame="${frame}"]` }, { kind: 'visible', selector: '#topbar .t', text: frame.startsWith('trace-') ? 'Trace' : title }];
  if (width > 390) assertions.push({ kind: 'visible', selector: '#nav a[href="/sessions"]', text: 'Sessions', inViewport: true }, { kind: 'visible', selector: '#nav a[href="/machines"]', text: 'Machines', inViewport: true });
  if (frame === 'sessions') {
    const rows = Object.entries(fixture.SESS).sort(([, a], [, b]) => b.last - a.last).map(([id, session]) => ({ attributes: { 'data-id': JSON.stringify(['laptop', id]) }, text: session.name }));
    assertions.push({ kind: 'visible', selector: '.session-list [data-session-row]', count: rows.length, rows }, { kind: 'visible', selector: '#sq[aria-label="Search session details"]' });
  }
  if (frame.startsWith('session-')) assertions.push({ kind: 'visible', selector: '#page section[aria-label="Transcript"]' });
  if (frame.startsWith('trace-')) assertions.push({ kind: 'visible', selector: '#page .trace-summary' }, { kind: 'visible', selector: '#topbar .crumb', text: title });
  if (frame === 'machine-desktop') assertions.push({ kind: 'visible', selector: '#page .nrow', count: 2, rows: [
    { attributes: { 'data-id': '5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a02' }, text: 'Release build' },
    { attributes: { 'data-id': '5e1c0a2e-0d1c-4b7e-9a51-2f6c1d0e7a01' }, text: 'Nightly review' },
  ] });
  return assertions;
}
function capture(comparator, scope, kind, legacyKey, frame, width, scheme, state = frame, regionName = null) {
  const spec = routeStories.find(([id]) => id === frame);
  if (!spec) throw Error('Unknown explicit legacy route ' + frame);
  const region = regionName ? regions[regionName] : null;
  if (regionName && !spec[3].includes(regionName)) throw Error('Unowned legacy region ' + frame + '/' + regionName);
  const legacy = `${comparator}:${scope}:${kind}:${legacyKey}`;
  const assertions = routeAssertions(frame, spec[2], width);
  if (state === 'drawer') assertions.push({ kind: 'visible', selector: '#sidebar' });
  if (state.includes('thinking') || state.includes('tools') || state.includes('tool-output')) assertions.push({ kind: 'visible', selector: '.think-text >> nth=0' });
  if (state === 'harbor-tools-open' || state === 'harbor-tool-output-open') assertions.push({ kind: 'visible', selector: '.tgroup .steps >> nth=0' });
  if (state === 'harbor-tool-output-open') assertions.push({ kind: 'visible', selector: '.tgroup .steps .out:not([hidden]) >> nth=0' });
  if (state === 'sessions-filter-open') assertions.push({ kind: 'focused', selector: '.filters-sheet[open] .vclose' });
  if (region) assertions.push({ kind: 'visible', selector: region.selector });
  return { key: 'legacy|' + legacy, legacyKey: legacy, storyId: story(spec[1]), width, scheme,
    fixture: { source: 'tests/ui/fixture-data/overhaul.json', scope, frame, revision: 'bf9190ef2b80fcd56726f61dc480346b698039a2', policyDependencies: ['tests/ui/fixture.mjs'] },
    args: { scope: scope === 'required' ? 'main' : scope },
    viewport: { width, height: width === 390 ? 844 : 860, deviceScaleFactor: width === 390 ? 2 : 1, isMobile: width === 390, hasTouch: width === 390 },
    capture: { selector: region?.selector ?? null, top: region?.top ?? true, viewportHeight: region?.viewportHeight ?? 'fixed',
      fullPage: comparator === 'pixels' && kind === 'frame' && width === 390, animations: comparator === 'preservation' || (comparator === 'pixels' && scope === 'main' && kind === 'frame') ? 'allow' : 'disabled', caret: 'hide',
      stablePaint: comparator === 'preservation' || (comparator === 'pixels' && scope === 'main' && kind === 'frame'), masks: [] },
    pixelPolicy: { threshold: 0.1, maxRatio: 0.0001 }, reducedMotion: 'no-preference', clock: '2026-09-28T12:40:00.000Z',
    actions: [...stateActions(state), ...(region?.open ? [{ kind: 'click', selector: region.open }] : [])], assertions,
    postCaptureActions: [], postCaptureAssertions: [], expectedState: `${scope}: ${frame}; ${state}${regionName ? '; region ' + regionName : ''}`,
    referenceDisposition: 'pending-storybook-review' };
}
export function legacyCapturePlan() {
  const plan = [];
  for (const { scope, frames, schemes } of scopes) for (const scheme of schemes) for (const frame of frames) {
    if (frame === 'drawer' && scheme.startsWith('desktop-')) continue;
    const width = scheme.startsWith('phone-') ? 390 : 1280, theme = scheme.endsWith('-dark') ? 'dark' : 'light';
    plan.push(capture('pixels', scope, 'frame', `${scheme}|${frame}`, frame, width, theme));
    for (const region of routeStories.find(([id]) => id === frame)[3])
      plan.push(capture('pixels', scope, 'region', `${scheme}|${frame}|${region}`, frame, width, theme, frame, region));
  }
  for (const width of [390, 820, 1280]) for (const theme of ['light', 'dark'])
    for (const [state, frame] of [...baseStates, ...(width === 390 ? [['drawer', 'drawer']] : [])])
      plan.push(capture('preservation', 'required', 'frame', `${width}-${theme}-${state}`, frame, width, theme, state));
  for (const width of [390, 820, 1280]) for (const scheme of ['light', 'dark']) for (const state of ['primitives', 'select', 'account', 'sheet', 'panel']) {
    const legacyKey = `gallery:required:frame:${state}-${width}-${scheme}`;
    const openSelect = [{ kind: 'focus', selector: '.sh-select-trigger' }, { kind: 'press', selector: '.sh-select-trigger', key: 'ArrowDown' }];
    const selectOwner = width === 390 ? '[role="listbox"]' : '.sh-select-trigger';
    // gallery.mjs captures the remaining overlays after choosing Oldest first.
    // Each isolated capture must replay that visible prerequisite too.
    const chooseOldest = [...openSelect, { kind: 'press', selector: selectOwner, key: 'ArrowDown' }, { kind: 'press', selector: selectOwner, key: 'Enter' }];
    const overlay = state === 'select' ? openSelect :
      state === 'account' ? [...(width === 390 ? [{ kind: 'click', selector: '#lead-btn' }] : []), { kind: 'click', selector: width === 390 ? '#account-drawer .account-trigger' : '#topbar .account-trigger' }] :
      state === 'sheet' || state === 'panel' ? [{ kind: 'click', selector: `#${state}-trigger` }] : [];
    const actions = [...(state === 'account' || state === 'sheet' || state === 'panel' ? chooseOldest : []), ...overlay];
    plan.push({ key: 'legacy|' + legacyKey, legacyKey, storyId: 'semon-legacy-gallery--gallery', width, scheme,
      fixture: { source: 'ui/tests/fixtures/design-gallery.tsx', scope: 'required', frame: state },
      viewport: { width, height: 844, deviceScaleFactor: width === 390 ? 2 : 1, isMobile: width === 390, hasTouch: width < 1000 },
      capture: { selector: null, top: true, viewportHeight: 'fixed', fullPage: false, animations: 'disabled', caret: 'hide', stablePaint: false, masks: [] },
      pixelPolicy: { threshold: 0.1, maxRatio: 0.001 }, reducedMotion: 'no-preference', actions,
      assertions: [{ kind: 'visible', selector: state === 'account' ? '.account-popover[role="menu"]' : state === 'sheet' || state === 'panel' ? 'dialog[open]' : state === 'select' ? '.sh-select-trigger[aria-expanded="true"]' : '#sheet-trigger' },
        ...(state === 'account' || state === 'sheet' || state === 'panel' ? [{ kind: 'visible', selector: '.sh-select-trigger', text: 'Oldest first' }] : [])],
      postCaptureActions: [], postCaptureAssertions: [], expectedState: 'Original gallery ' + state, referenceDisposition: 'pending-storybook-review' });
  }
  return plan;
}

export function successorErrors(expectedKeys, plan) {
  const expected = new Set(expectedKeys), seen = new Set(), captures = new Set(), errors = [];
  const declared = new Map(legacyCapturePlan().map((item) => [item.legacyKey, item]));
  for (const item of plan.filter((row) => row.legacyKey)) {
    if (!expected.has(item.legacyKey)) errors.push('Unknown legacy successor ' + item.legacyKey);
    if (seen.has(item.legacyKey)) errors.push('Duplicate legacy successor ' + item.legacyKey);
    if (captures.has(item.key)) errors.push('Duplicate successor capture ' + item.key);
    seen.add(item.legacyKey); captures.add(item.key);
    if (!item.fixture?.source || !item.fixture.scope || !item.fixture.frame || !item.viewport?.height || !item.viewport.deviceScaleFactor || !item.capture || !item.expectedState || item.referenceDisposition !== 'pending-storybook-review') errors.push('Incomplete exact successor identity ' + item.key);
    const binding = declared.get(item.legacyKey);
    if (binding && JSON.stringify(item) !== JSON.stringify(binding)) errors.push('Changed explicit legacy successor binding ' + item.legacyKey);
  }
  for (const key of expected) if (!seen.has(key)) errors.push('Missing Storybook successor ' + key);
  return errors;
}
