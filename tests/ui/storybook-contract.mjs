import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { makeCapturePlan } from './storybook-plan.mjs';
import { referenceErrors } from './storybook-references.mjs';
import {
  EXPECTED,
  REFERENCE,
  THRESHOLD,
  MAX_RATIO,
  expectedPixelCoverage,
} from './preservation-contract.mjs';
import { successorErrors, routeStoryIds, routeStories } from './storybook-legacy.mjs';
import { GROUPS } from './suite-plan.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const readJson = (file) => JSON.parse(fs.readFileSync(path.resolve(here, file), 'utf8'));
const contract = readJson('storybook-contract.json');
const inventory = readJson(contract.stateBindings);
const errors = [],
  mappedStories = new Set(),
  mappedChecks = new Set();
const storySource = fs.readFileSync(
  path.join(root, 'ui/stories/ProductionSurfaces.stories.tsx'),
  'utf8',
);
const gallerySource = fs.readFileSync(
  path.join(root, 'ui/tests/fixtures/design-gallery.tsx'),
  'utf8',
);
const libraryIndex = fs.readFileSync(path.join(root, 'ui/src/lib/index.ts'), 'utf8');
const storyIndex = readJson(contract.storyIndex);
const availableStories = new Set(
  Object.entries(storyIndex.entries ?? {})
    .filter(
      ([id, entry]) =>
        entry.type === 'story' &&
        (contract.ownedStoryPrefixes ?? [contract.ownedStoryPrefix]).some((prefix) =>
          id.startsWith(prefix),
        ),
    )
    .map(([id]) => id),
);
const sameSet = (left, right) =>
  Array.isArray(left) &&
  new Set(left).size === left.length &&
  [...left].sort().join('\n') === [...right].sort().join('\n');
function addStories(ids, source) {
  if (!Array.isArray(ids) || !ids.length) {
    errors.push(`${source}: explicit Storybook owner required`);
    return;
  }
  for (const id of ids) {
    if (!availableStories.has(id)) errors.push(`${source}: unknown story ${id}`);
    mappedStories.add(id);
  }
}
function addCheck(id, source) {
  if (!Object.hasOwn(contract.servedChecks, id))
    errors.push(`${source}: unknown served check ${id}`);
  else mappedChecks.add(id);
}
if (contract.version !== 2 || inventory.version !== 1)
  errors.push('Unsupported migration contract version');
if (
  contract.legacySuccessors !== 'storybook-legacy.mjs' ||
  contract.gateInventory !== 'storybook-gates.json' ||
  !sameSet(contract.ownedStoryPrefixes, [
    'semon-production-owners--',
    'semon-legacy-routes--',
    'semon-legacy-gallery--',
  ])
)
  errors.push('Complete Storybook successor/gate ownership is required');
if (
  contract.pixelPolicy.threshold !== THRESHOLD ||
  contract.pixelPolicy.maxRatio !== MAX_RATIO ||
  !sameSet(contract.pixelPolicy.dimensions, [390, 820, 1280]) ||
  !sameSet(contract.pixelPolicy.schemes, ['light', 'dark'])
)
  errors.push('Storybook must retain the strict pixel policy and full default matrix');
for (const [id, file] of Object.entries(contract.servedChecks))
  if (!fs.existsSync(path.resolve(here, file))) errors.push(`Missing check file ${id}: ${file}`);
// Existence alone accepted the constants-only module as a capture runner.
if (
  contract.servedChecks.preservation !== 'preservation.mjs' ||
  contract.preservation.source !== 'preservation-contract.mjs'
)
  errors.push('Preservation executable owner/policy dependency changed');
const preservationRunner = fs.readFileSync(path.join(here, 'preservation.mjs'), 'utf8');
if (
  !preservationRunner.includes('captureStableFrame(') ||
  !preservationRunner.includes("from './preservation-contract.mjs'")
)
  errors.push('Preservation owner is not the executable capture runner');
for (const surface of contract.productionSurfaces) {
  addStories(surface.storyIds, `surface ${surface.id}`);
  for (const id of surface.servedChecks ?? []) addCheck(id, `surface ${surface.id}`);
  for (const module of surface.modules ?? []) {
    const file = path.resolve(root, 'ui/stories', module),
      name = path.basename(module);
    if (!['', '.ts', '.tsx', '.js', '.mjs'].some((extension) => fs.existsSync(file + extension)))
      errors.push(`surface ${surface.id}: missing owner ${module}`);
    const direct =
      storySource.includes(`../src/lib/${name}`) || storySource.includes(`../src/app/${name}`);
    const barrel =
      gallerySource.includes("from '../../src/lib'") && libraryIndex.includes(`from './${name}'`);
    if (!direct && !barrel)
      errors.push(`surface ${surface.id}: production owner is not imported: ${module}`);
  }
}
let plan = [];
try {
  plan = makeCapturePlan(contract, inventory, [...availableStories]);
} catch (error) {
  errors.push(error.message);
}
for (const interaction of contract.interactionCaptures ?? [])
  addStories([interaction.storyId], `interaction ${interaction.id}`);
const planByKey = new Map(plan.map((item) => [item.key, item]));
addStories(
  [...routeStoryIds, 'semon-legacy-gallery--gallery'],
  'complete legacy presentation owners',
);
const routeExports = fs.readFileSync(path.join(root, 'ui/stories/LegacyRoutes.stories.tsx'), 'utf8');
if (!routeExports.includes("from './LegacyNativeRoute'") || !routeExports.includes('component: NativeRouteOwner'))
  errors.push('Legacy exports lost the native presentation composition');
const routeSource = fs.readFileSync(path.join(root, 'ui/stories/LegacyNativeRoute.tsx'), 'utf8');
for (const owner of [
  'createShellChrome',
  'createRecentRenderer',
  'createViewerBar',
  'createOverviewPages',
  'renderSessionScreen',
  'renderCausalTrace',
  'renderCatalogList',
  'createCatalogBrowser',
  'renderSessionMenu',
  'releaseScreen',
])
  if (!routeSource.includes(owner + '('))
    errors.push('Missing production route presentation owner ' + owner);
const overviewSource = fs.readFileSync(path.join(root, 'ui/src/app/overviewPages.tsx'), 'utf8');
for (const owner of ['renderHomeScreen', 'renderMachinesScreen', 'renderMachineScreen', 'renderAnalyticsPresentation', 'selectRecentChildren', 'selectRecentRoots'])
  if (!overviewSource.includes(owner + '(')) errors.push('Native overview lost production presentation owner ' + owner);
const nativeRecords = JSON.parse(fs.readFileSync(path.join(root, 'ui/stories/LegacyNative.records.json'), 'utf8'));
if (nativeRecords.version !== 1 || !/^[a-f0-9]{40}$/.test(nativeRecords.sourceRevision ?? '') ||
    (process.env.GITHUB_SHA && nativeRecords.sourceRevision !== process.env.GITHUB_SHA))
  errors.push('Legacy native fixture records must be generated from this exact CI head before Storybook build');
for (const scope of ['main', 'two-machines', 'pushed']) {
  const expectedFrames = new Set(plan.filter((item) => item.legacyKey && !item.legacyKey.startsWith('gallery:') &&
    (item.fixture.scope === scope || (scope === 'main' && item.fixture.scope === 'required'))).map((item) => item.fixture.frame));
  const records = nativeRecords.scopes?.[scope] ?? {};
  if (!sameSet(expectedFrames, Object.keys(records))) errors.push('Missing/unknown native fixture frames in scope ' + scope);
  for (const [frame, record] of Object.entries(records))
    if (!Array.isArray(record.responses) || !record.responses.length || !record.route)
      errors.push('Incomplete native fixture publication ' + scope + '/' + frame);
}
const filterOwner = fs.readFileSync(path.join(root, 'ui/src/app/catalogBrowser.ts'), 'utf8');
for (const owner of ['createFacetChrome', 'createSelect'])
  if (!filterOwner.includes(owner + '('))
    errors.push('Shared route filter owner lost production control ' + owner);
if (/\b(?:mountViewerApplication|createCatalogViewer)\s*\(/.test(routeSource + routeExports))
  errors.push('Route presentation stories must not mount transport/application owners');
const retained = readJson('fixture-data/overhaul.json');
if (
  !sameSet(
    routeStories.filter(([frame]) => frame.startsWith('session-')).map(([frame]) => frame.slice(8)),
    Object.keys(retained.TX),
  )
)
  errors.push('Named session stories must cover the entire retained transcript fixture');
for (const [frame, , title] of routeStories.filter(([frame]) => frame.startsWith('session-')))
  if (retained.SESS[frame.slice(8)]?.name !== title)
    errors.push('Named route story lost its retained fixture identity ' + frame);

// Exact existing comparator keys remain independent captures. Never map a
// machine detail or transcript route to a visually unrelated component story.
const servedCases = new Map();
function servedCase(comparator, scope, kind, key, check, details) {
  const id = `${comparator}:${scope}:${kind}:${key}`;
  if (servedCases.has(id)) errors.push(`duplicate served capture ${id}`);
  servedCases.set(id, { comparator, scope, kind, key, servedCheck: check, ...details });
  addCheck(check, id);
}
const visual = readJson(contract.visualCoverage.source);
if (!sameSet(contract.visualCoverage.frames, visual.frames))
  errors.push('Exact visual frame inventory has missing/unknown/duplicate routes');
for (const field of ['scopes', 'regions'])
  if (JSON.stringify(contract.visualCoverage[field]) !== JSON.stringify(visual[field]))
    errors.push(`Exact visual ${field} inventory differs from the served manifest`);
for (const scope of Object.keys(visual.scopes)) {
  const coverage = expectedPixelCoverage(scope);
  for (const kind of ['frame', 'region'])
    for (const key of coverage[kind === 'frame' ? 'frames' : 'regions']) {
      const [scheme, frame, region] = key.split('|');
      servedCase('pixels', scope, kind, key, contract.visualCoverage.servedCheck, {
        viewport: scheme.startsWith('phone-') ? [390, 844] : [1280, 860],
        theme: scheme.endsWith('-dark') ? 'dark' : 'light',
        frame,
        region: region ?? null,
        expectedState: `Exact served ${frame}${region ? ` region ${region}` : ''}; ${contract.visualCoverage.expectedState}`,
        referenceDisposition: contract.visualCoverage.referenceDisposition,
        reference: scope === 'pushed' ? 'served-parity' : REFERENCE,
      });
    }
}
const declaredPreservation = contract.preservation.widths.flatMap((width) =>
  contract.preservation.schemes.flatMap((theme) =>
    [
      ...contract.preservation.states,
      ...(width === 390 ? contract.preservation.phoneOnlyStates : []),
    ].map((state) => `${width}-${theme}-${state}`),
  ),
);
if (!sameSet(declaredPreservation, EXPECTED))
  errors.push('Strict preservation inventory has missing/unknown/duplicate states');
for (const key of EXPECTED) {
  const [, width, theme, state] = /^(\d+)-(light|dark)-(.+)$/.exec(key);
  servedCase('preservation', 'required', 'frame', key, contract.preservation.servedCheck, {
    viewport: [Number(width), Number(width) === 390 ? 844 : 860],
    theme,
    expectedState: `Exact strict-preservation state ${state}, including its existing interaction setup`,
    referenceDisposition: contract.preservation.referenceDisposition,
    reference: REFERENCE,
  });
}
const acceptance = readJson(contract.visualAcceptance.source),
  acceptanceKeys = new Set();
for (const item of acceptance.acceptances ?? []) {
  const key = `${item.comparator}:${item.scope}:${item.kind}:${item.key}`;
  if (acceptanceKeys.has(key)) errors.push(`duplicate visual acceptance ${key}`);
  if (!servedCases.has(key)) errors.push(`visual acceptance has no exact served capture: ${key}`);
  acceptanceKeys.add(key);
  addCheck(contract.visualAcceptance.comparatorChecks[item.comparator], key);
}
const gallery = readJson(contract.gallery.source);
if (!sameSet(contract.gallery.states, gallery.states))
  errors.push('Gallery state inventory has missing/unknown/duplicate states');
for (const field of ['widths', 'schemes', 'threshold', 'maxRatio'])
  if (JSON.stringify(contract.gallery[field]) !== JSON.stringify(gallery[field]))
    errors.push(`Gallery ${field} differs from its existing comparator`);
for (const state of gallery.states)
  for (const width of gallery.widths)
    for (const theme of gallery.schemes)
      servedCase(
        'gallery',
        'required',
        'frame',
        `${state}-${width}-${theme}`,
        contract.gallery.servedCheck,
        {
          viewport: [width, 844],
          theme,
          expectedState: `Production gallery ${state}; existing keyboard, touch and overlay sequence`,
          referenceDisposition: 'preserve-existing-gallery-baseline',
          reference: 'baselines/gallery',
          threshold: gallery.threshold,
          maxRatio: gallery.maxRatio,
        },
      );
addStories(contract.gallery.storyIds, 'gallery');
errors.push(...successorErrors([...servedCases.keys()], plan));
const successors = new Map(
  plan.filter((item) => item.legacyKey).map((item) => [item.legacyKey, item]),
);
for (const item of plan.filter((row) => row.legacyKey)) {
  const old = servedCases.get(item.legacyKey);
  if (!old) continue;
  if (
    item.width !== old.viewport[0] ||
    item.viewport.height !== old.viewport[1] ||
    item.scheme !== old.theme ||
    item.viewport.deviceScaleFactor !== (item.width === 390 ? 2 : 1)
  )
    errors.push('Changed legacy viewport/theme/scale ' + item.legacyKey);
  if (
    item.pixelPolicy.threshold !== (old.threshold ?? THRESHOLD) ||
    item.pixelPolicy.maxRatio !== (old.maxRatio ?? MAX_RATIO)
  )
    errors.push('Changed legacy successor pixel policy ' + item.legacyKey);
  if (
    old.region &&
    item.capture.selector !== visual.regions.find((row) => row.name === old.region)?.selector
  )
    errors.push('Changed exact legacy region selector ' + item.legacyKey);
}

const gates = readJson(contract.gateInventory);
const workflow = fs.readFileSync(path.join(root, gates.authority), 'utf8');
const matrix = /suite:\s*\[([^\]]+)\]/
  .exec(workflow)?.[1]
  .split(',')
  .map((item) => item.trim());
if (
  gates.version !== 1 ||
  !sameSet(gates.matrix, matrix ?? []) ||
  gates.aggregate.check !== 'viewer browser checks' ||
  !workflow.includes('name: ' + gates.aggregate.check)
)
  errors.push('Native workflow/matrix gate inventory differs from its authority');
const aggregateNeeds = /\n  browser:[\s\S]*?needs:\s*\[([^\]]+)\]/
  .exec(workflow)?.[1]
  .split(',')
  .map((item) => item.trim());
if (
  !sameSet(gates.aggregate.needs, aggregateNeeds ?? []) ||
  !['foundation', 'build', 'checks'].every((id) => aggregateNeeds?.includes(id))
)
  errors.push('Required native aggregate dependencies differ from the gate inventory');
const gateIds = new Set();
for (const gate of gates.gates ?? []) {
  if (
    !gate.id ||
    gateIds.has(gate.id) ||
    !gate.owner ||
    !['active', 'conditional'].includes(gate.status) ||
    !gate.disposition ||
    !Object.hasOwn(gate, 'successor')
  )
    errors.push('Invalid/duplicate native gate disposition ' + gate.id);
  gateIds.add(gate.id);
  const file = gate.owner.split('#')[0];
  if (!fs.existsSync(path.resolve(root, file))) errors.push('Missing native gate owner ' + file);
  if (gate.id === 'build' && JSON.stringify(gate.reference) !== JSON.stringify(REFERENCE))
    errors.push('Build reference provenance differs from preservation policy');
  if (
    !file.startsWith('.github/') &&
    !gate.id.startsWith('native-') &&
    !workflow.includes(path.basename(file))
  )
    errors.push('Native gate executable is absent from the workflow ' + file);
  if (gate.id === 'functional') {
    if (!sameSet(Object.keys(gate.groups), Object.keys(GROUPS)))
      errors.push('Functional group inventory changed');
    for (const [group, members] of Object.entries(GROUPS))
      if (!sameSet(gate.groups[group], members))
        errors.push('Functional group members differ: ' + group);
  }
  for (const member of gate.members ?? [])
    if (!fs.existsSync(path.resolve(root, member)) || !workflow.includes(path.basename(member)))
      errors.push('Native gate member is not executed by its workflow: ' + member);
  if (gate.id.startsWith('native-')) {
    const pkg = JSON.parse(fs.readFileSync(path.join(root, 'ui/package.json'), 'utf8'));
    if (
      gate.job !== 'foundation' ||
      pkg.scripts.test !== 'node --test tests/*.test.mjs' ||
      !gate.owner.startsWith('ui/tests/') ||
      !gate.owner.endsWith('.test.mjs')
    )
      errors.push('Native unit owner is not executed by foundation wildcard ' + gate.id);
  }
  if (
    gate.status === 'conditional' &&
    (!gate.condition || !workflow.includes('inputs.refresh_diagnostics'))
  )
    errors.push('Conditional gate lost its execution condition ' + gate.id);
}
for (const id of [
  'foundation',
  'build',
  'functional',
  'independent-interaction',
  'native-navigation',
  'native-selection',
  'gaps',
  'strict-preservation',
  'strict-pixels',
  'live',
  'errors',
  'performance',
  'budget',
  'order',
  'pushed-parity',
  'refresh-diagnostics',
  'storybook',
])
  if (!gateIds.has(id)) errors.push('Missing native gate disposition ' + id);
if (
  !sameSet(
    (gates.masks ?? []).map((entry) => entry.owner),
    [
      'tests/ui/pixels.mjs',
      'tests/ui/preservation.mjs',
      'tests/ui/gallery.mjs',
      'tests/ui/compact-gallery.mjs',
      'ui/tests/catalog-served.mjs',
      'ui/tests/overview-served.mjs',
      'tests/ui/storybook-capture.mjs',
    ],
  )
)
  errors.push('Missing/unknown/duplicate inspected mask owner');
for (const entry of gates.masks ?? []) {
  const source = fs.readFileSync(path.resolve(root, entry.owner), 'utf8');
  if (
    !Array.isArray(entry.masks) ||
    entry.masks.length ||
    !entry.evidence ||
    !entry.disposition ||
    !Object.hasOwn(entry, 'successor')
  )
    errors.push('Invalid inspected no-mask evidence ' + entry.owner);
  if (/\bmask\s*:/.test(source))
    errors.push('Capture owner acquired a mask; inventory requires explicit review ' + entry.owner);
}
if (
  !sameSet(
    (gates.historical ?? []).map((entry) => entry.owner),
    [
      'tests/ui/pixel-baseline.json',
      'tests/ui/historical-reference-map.json',
      'tests/ui/historical-pixel-ratchet.json',
    ],
  )
)
  errors.push('Missing/unknown/duplicate historical provenance disposition');
for (const entry of gates.historical ?? []) {
  const authorityField = entry.authority?.split('#')[1];
  if (
    entry.status !== 'inactive' ||
    !entry.disposition ||
    !Object.hasOwn(entry, 'successor') ||
    !visual[authorityField]?.includes(path.basename(entry.owner))
  )
    errors.push('Historical disposition lacks its source authority ' + entry.owner);
}

const pen = readJson(contract.pen.source),
  frameById = new Map();
function collectFrames(nodes, parent = null) {
  for (const node of nodes ?? []) {
    if (node.type === 'frame' && typeof node.id === 'string') {
      if (frameById.has(node.id)) errors.push(`duplicate pen frame ${node.id}`);
      frameById.set(node.id, { node, parent });
      collectFrames(node.children, node.id);
    } else collectFrames(node.children, parent);
  }
}
collectFrames(pen.children);
// Adding a second canvas must require a deliberate inventory update as well.
function penFiles(directory) {
  return fs.readdirSync(directory, { withFileTypes: true }).flatMap((entry) => {
    const file = path.join(directory, entry.name);
    return entry.isDirectory() ? penFiles(file) : file.endsWith('.pen') ? [file] : [];
  });
}
if (!sameSet(penFiles(path.join(root, 'docs/design')), [path.resolve(here, contract.pen.source)]))
  errors.push('Unknown or missing design canvas: update the full migration inventory');
const activeIds = new Set(),
  supersededIds = new Set();
const roots = new Set(
  (pen.children ?? []).filter((node) => node.type === 'frame').map((node) => node.id),
);
if (
  !sameSet(
    contract.pen.roots.map((entry) => entry.id),
    [...roots],
  )
)
  errors.push('Missing/unknown/duplicate pen root disposition');
for (const entry of contract.pen.roots) {
  if (!['active', 'superseded'].includes(entry.disposition)) {
    errors.push(`invalid disposition ${entry.id}`);
    continue;
  }
  const descendants = [...frameById]
    .filter(([, value]) => {
      let parent = value.parent;
      while (parent) {
        if (parent === entry.id) return true;
        parent = frameById.get(parent)?.parent;
      }
      return false;
    })
    .map(([id]) => id);
  const groups = (
    entry.disposition === 'active'
      ? contract.pen.activeDescendantGroups
      : contract.pen.supersededDescendantGroups
  ).filter((group) => group.root === entry.id);
  if (groups.length !== 1 || !sameSet(groups[0]?.ids, descendants))
    errors.push(`pen root ${entry.id}: incomplete/unknown/duplicate descendant classification`);
  if (
    entry.disposition === 'superseded' &&
    (!entry.reason ||
      !entry.decisionSource ||
      !fs.existsSync(path.resolve(root, entry.decisionSource)))
  )
    errors.push(`superseded pen root ${entry.id}: governing decision source and reason required`);
  const set = entry.disposition === 'active' ? activeIds : supersededIds;
  for (const id of [entry.id, ...descendants]) set.add(id);
}
for (const group of [
  ...contract.pen.activeDescendantGroups,
  ...contract.pen.supersededDescendantGroups,
])
  if (!roots.has(group.root)) errors.push(`unknown descendant classification root ${group.root}`);
const retiredFrames = new Map();
for (const disposition of inventory.retiredFrames ?? []) {
  if (
    disposition.disposition !== 'superseded' ||
    !disposition.reason?.trim() ||
    !disposition.decisionSource ||
    !fs.existsSync(path.resolve(root, disposition.decisionSource)) ||
    !disposition.frames?.length
  )
    errors.push(
      'Retired descendant requires explicit IDs, a governing decision source and a reason',
    );
  for (const id of disposition.frames ?? []) {
    if (!activeIds.has(id) || roots.has(id) || retiredFrames.has(id))
      errors.push(`unknown/duplicate/non-descendant retirement ${id}`);
    activeIds.delete(id);
    supersededIds.add(id);
    retiredFrames.set(id, {
      disposition: disposition.disposition,
      reason: disposition.reason,
      decisionSource: disposition.decisionSource,
    });
  }
}
for (const id of frameById.keys())
  if (activeIds.has(id) === supersededIds.has(id))
    errors.push(`unclassified/ambiguous pen frame ${id}`);

const resolvedTargets = new Map();
for (const [id, target] of Object.entries(inventory.targets)) {
  const types = ['storyId', 'interactionId', 'legacyKeys'].filter((key) =>
    Object.hasOwn(target, key),
  );
  if (types.length !== 1) {
    errors.push(`target ${id}: exactly one executable owner required`);
    continue;
  }
  let keys = [];
  if (target.storyId) {
    addStories([target.storyId], `target ${id}`);
    keys = target.widths.flatMap((width) =>
      target.schemes.map((theme) => `${target.storyId}|${width}|${theme}`),
    );
    for (const key of keys)
      if (!planByKey.has(key)) errors.push(`target ${id}: missing executable story capture ${key}`);
  } else if (target.interactionId) {
    keys = plan
      .filter((item) => item.interactionId === target.interactionId)
      .map((item) => item.key);
    if (!keys.length)
      errors.push(`target ${id}: unknown/missing interaction ${target.interactionId}`);
  } else {
    if (
      !target.legacyKeys?.length ||
      !target.expectedState ||
      target.referenceDisposition !== 'pending-storybook-review'
    )
      errors.push(
        `target ${id}: exact legacy successor keys, expected state and pending disposition required`,
      );
    keys = (target.legacyKeys ?? []).map((key) => successors.get(key)?.key);
    for (const key of keys)
      if (!planByKey.has(key))
        errors.push(`target ${id}: missing executable Storybook successor ${key}`);
  }
  resolvedTargets.set(id, keys);
}
const frameBindings = new Map(),
  usedTargets = new Set();
for (const binding of inventory.bindings) {
  if (!binding.frames?.length || !binding.targets?.length || !binding.visibleOwnership?.trim())
    errors.push('Every binding needs explicit frame IDs, capture targets and visible ownership');
  const keys = [];
  for (const target of binding.targets ?? []) {
    usedTargets.add(target);
    if (!resolvedTargets.has(target)) errors.push(`binding: unknown capture target ${target}`);
    keys.push(...(resolvedTargets.get(target) ?? []));
  }
  if (!keys.length || keys.some((key) => !planByKey.has(key)))
    errors.push('Active pen binding must resolve entirely to Storybook captures');
  for (const id of binding.frames ?? []) {
    if (!activeIds.has(id)) errors.push(`binding claims unknown/inactive pen frame ${id}`);
    if (frameBindings.has(id)) errors.push(`duplicate executable pen binding ${id}`);
    frameBindings.set(id, {
      name: frameById.get(id)?.node.name,
      captures: keys,
      visibleOwnership: binding.visibleOwnership,
    });
  }
}
for (const id of activeIds)
  if (!frameBindings.has(id)) errors.push(`active pen frame lacks an executable binding: ${id}`);
for (const id of resolvedTargets.keys())
  if (!usedTargets.has(id)) errors.push(`unused capture target ${id}`);
for (const interaction of contract.interactionCaptures ?? [])
  for (const frame of interaction.frames ?? []) {
    const captures = frameBindings.get(frame)?.captures ?? [];
    if (!plan.some((item) => item.interactionId === interaction.id && captures.includes(item.key)))
      errors.push(
        `interaction ${interaction.id}: frame ${frame} has no matching executable binding`,
      );
  }
for (const id of availableStories) if (!mappedStories.has(id)) errors.push(`orphaned story ${id}`);
const approved = readJson(contract.referenceMap);
errors.push(...referenceErrors(approved, plan));

const report = {
  activeStories: [...mappedStories].sort(),
  servedChecks: [...mappedChecks].sort(),
  executableStoryCaptures: plan.map((item) => ({
    ...item,
    referenceDisposition: 'pending-storybook-review',
  })),
  exactServedCaptures: [...servedCases.values()],
  visualAcceptances: [...acceptanceKeys],
  legacySuccessors: Object.fromEntries([...successors].map(([key, item]) => [key, item.key])),
  gateInventory: gates,
  penFrames: {
    active: activeIds.size,
    superseded: supersededIds.size,
    executable: frameBindings.size,
    inherited: 0,
  },
  frameBindings: Object.fromEntries(frameBindings),
  errors,
  retiredFrames: Object.fromEntries(retiredFrames),
};
const out = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook');
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'contract-report.json'), JSON.stringify(report, null, 2) + '\n');
if (errors.length) {
  console.error(errors.join('\n'));
  process.exitCode = 1;
} else
  console.log(
    `Storybook contract: ${mappedStories.size} stories, ${plan.length} executable captures, ${servedCases.size} exact served states, ${frameBindings.size} explicit pen bindings; no inherited coverage.`,
  );
