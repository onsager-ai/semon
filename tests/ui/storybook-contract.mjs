import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { makeCapturePlan } from './storybook-plan.mjs';
import { referenceErrors } from './storybook-references.mjs';
import { EXPECTED, REFERENCE, THRESHOLD, MAX_RATIO, expectedPixelCoverage } from './preservation-contract.mjs';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const readJson = (file) => JSON.parse(fs.readFileSync(path.resolve(here, file), 'utf8'));
const contract = readJson('storybook-contract.json');
const inventory = readJson(contract.stateBindings);
const errors = [], mappedStories = new Set(), mappedChecks = new Set();
const storySource = fs.readFileSync(path.join(root, 'ui/stories/ProductionSurfaces.stories.tsx'), 'utf8');
const gallerySource = fs.readFileSync(path.join(root, 'ui/tests/fixtures/design-gallery.tsx'), 'utf8');
const libraryIndex = fs.readFileSync(path.join(root, 'ui/src/lib/index.ts'), 'utf8');
const storyIndex = readJson(contract.storyIndex);
const availableStories = new Set(Object.entries(storyIndex.entries ?? {})
  .filter(([id, entry]) => entry.type === 'story' && id.startsWith(contract.ownedStoryPrefix)).map(([id]) => id));
const sameSet = (left, right) => Array.isArray(left) && new Set(left).size === left.length &&
  [...left].sort().join('\n') === [...right].sort().join('\n');
function addStories(ids, source) {
  if (!Array.isArray(ids) || !ids.length) { errors.push(`${source}: explicit Storybook owner required`); return; }
  for (const id of ids) {
    if (!availableStories.has(id)) errors.push(`${source}: unknown story ${id}`);
    mappedStories.add(id);
  }
}
function addCheck(id, source) {
  if (!Object.hasOwn(contract.servedChecks, id)) errors.push(`${source}: unknown served check ${id}`);
  else mappedChecks.add(id);
}
if (contract.version !== 2 || inventory.version !== 1) errors.push('Unsupported migration contract version');
if (contract.pixelPolicy.threshold !== THRESHOLD || contract.pixelPolicy.maxRatio !== MAX_RATIO ||
    !sameSet(contract.pixelPolicy.dimensions, [390, 820, 1280]) || !sameSet(contract.pixelPolicy.schemes, ['light', 'dark']))
  errors.push('Storybook must retain the strict pixel policy and full default matrix');
for (const [id, file] of Object.entries(contract.servedChecks))
  if (!fs.existsSync(path.resolve(here, file))) errors.push(`Missing check file ${id}: ${file}`);
for (const surface of contract.productionSurfaces) {
  addStories(surface.storyIds, `surface ${surface.id}`);
  for (const id of surface.servedChecks ?? []) addCheck(id, `surface ${surface.id}`);
  for (const module of surface.modules ?? []) {
    const file = path.resolve(root, 'ui/stories', module), name = path.basename(module);
    if (!['', '.ts', '.tsx', '.js', '.mjs'].some((extension) => fs.existsSync(file + extension)))
      errors.push(`surface ${surface.id}: missing owner ${module}`);
    const direct = storySource.includes(`../src/lib/${name}`) || storySource.includes(`../src/app/${name}`);
    const barrel = gallerySource.includes("from '../../src/lib'") && libraryIndex.includes(`from './${name}'`);
    if (!direct && !barrel) errors.push(`surface ${surface.id}: production owner is not imported: ${module}`);
  }
}
let plan = [];
try { plan = makeCapturePlan(contract, inventory, [...availableStories]); }
catch (error) { errors.push(error.message); }
for (const interaction of contract.interactionCaptures ?? []) addStories([interaction.storyId], `interaction ${interaction.id}`);
const planByKey = new Map(plan.map((item) => [item.key, item]));

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
if (!sameSet(contract.visualCoverage.frames, visual.frames)) errors.push('Exact visual frame inventory has missing/unknown/duplicate routes');
for (const field of ['scopes', 'regions'])
  if (JSON.stringify(contract.visualCoverage[field]) !== JSON.stringify(visual[field])) errors.push(`Exact visual ${field} inventory differs from the served manifest`);
for (const scope of Object.keys(visual.scopes)) {
  const coverage = expectedPixelCoverage(scope);
  for (const kind of ['frame', 'region']) for (const key of coverage[kind === 'frame' ? 'frames' : 'regions']) {
    const [scheme, frame, region] = key.split('|');
    servedCase('pixels', scope, kind, key, contract.visualCoverage.servedCheck, {
      viewport: scheme.startsWith('phone-') ? [390, 844] : [1280, 860],
      theme: scheme.endsWith('-dark') ? 'dark' : 'light', frame, region: region ?? null,
      expectedState: `Exact served ${frame}${region ? ` region ${region}` : ''}; ${contract.visualCoverage.expectedState}`,
      referenceDisposition: contract.visualCoverage.referenceDisposition,
      reference: scope === 'pushed' ? 'served-parity' : REFERENCE,
    });
  }
}
const declaredPreservation = contract.preservation.widths.flatMap((width) => contract.preservation.schemes.flatMap((theme) =>
  [...contract.preservation.states, ...(width === 390 ? contract.preservation.phoneOnlyStates : [])].map((state) => `${width}-${theme}-${state}`)));
if (!sameSet(declaredPreservation, EXPECTED)) errors.push('Strict preservation inventory has missing/unknown/duplicate states');
for (const key of EXPECTED) {
  const [, width, theme, state] = /^(\d+)-(light|dark)-(.+)$/.exec(key);
  servedCase('preservation', 'required', 'frame', key, contract.preservation.servedCheck, {
    viewport: [Number(width), Number(width) === 390 ? 844 : 860], theme,
    expectedState: `Exact strict-preservation state ${state}, including its existing interaction setup`,
    referenceDisposition: contract.preservation.referenceDisposition, reference: REFERENCE,
  });
}
const acceptance = readJson(contract.visualAcceptance.source), acceptanceKeys = new Set();
for (const item of acceptance.acceptances ?? []) {
  const key = `${item.comparator}:${item.scope}:${item.kind}:${item.key}`;
  if (acceptanceKeys.has(key)) errors.push(`duplicate visual acceptance ${key}`);
  if (!servedCases.has(key)) errors.push(`visual acceptance has no exact served capture: ${key}`);
  acceptanceKeys.add(key);
  addCheck(contract.visualAcceptance.comparatorChecks[item.comparator], key);
}
const gallery = readJson(contract.gallery.source);
if (!sameSet(contract.gallery.states, gallery.states)) errors.push('Gallery state inventory has missing/unknown/duplicate states');
for (const field of ['widths', 'schemes', 'threshold', 'maxRatio'])
  if (JSON.stringify(contract.gallery[field]) !== JSON.stringify(gallery[field])) errors.push(`Gallery ${field} differs from its existing comparator`);
for (const state of gallery.states) for (const width of gallery.widths) for (const theme of gallery.schemes)
  servedCase('gallery', 'required', 'frame', `${state}-${width}-${theme}`, contract.gallery.servedCheck, {
    viewport: [width, 844], theme, expectedState: `Production gallery ${state}; existing keyboard, touch and overlay sequence`,
    referenceDisposition: 'preserve-existing-gallery-baseline', reference: 'baselines/gallery',
    threshold: gallery.threshold, maxRatio: gallery.maxRatio,
  });
addStories(contract.gallery.storyIds, 'gallery');

const pen = readJson(contract.pen.source), frameById = new Map();
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
const activeIds = new Set(), supersededIds = new Set();
const roots = new Set((pen.children ?? []).filter((node) => node.type === 'frame').map((node) => node.id));
if (!sameSet(contract.pen.roots.map((entry) => entry.id), [...roots])) errors.push('Missing/unknown/duplicate pen root disposition');
for (const entry of contract.pen.roots) {
  if (!['active', 'superseded'].includes(entry.disposition)) { errors.push(`invalid disposition ${entry.id}`); continue; }
  const descendants = [...frameById].filter(([, value]) => {
    let parent = value.parent;
    while (parent) { if (parent === entry.id) return true; parent = frameById.get(parent)?.parent; }
    return false;
  }).map(([id]) => id);
  const groups = (entry.disposition === 'active' ? contract.pen.activeDescendantGroups : contract.pen.supersededDescendantGroups)
    .filter((group) => group.root === entry.id);
  if (groups.length !== 1 || !sameSet(groups[0]?.ids, descendants)) errors.push(`pen root ${entry.id}: incomplete/unknown/duplicate descendant classification`);
  if (entry.disposition === 'superseded' && (!entry.reason || !entry.decisionSource || !fs.existsSync(path.resolve(root, entry.decisionSource))))
    errors.push(`superseded pen root ${entry.id}: governing decision source and reason required`);
  const set = entry.disposition === 'active' ? activeIds : supersededIds;
  for (const id of [entry.id, ...descendants]) set.add(id);
}
for (const group of [...contract.pen.activeDescendantGroups, ...contract.pen.supersededDescendantGroups])
  if (!roots.has(group.root)) errors.push(`unknown descendant classification root ${group.root}`);
const retiredFrames = new Map();
for (const disposition of inventory.retiredFrames ?? []) {
  if (disposition.disposition !== 'superseded' || !disposition.reason?.trim() ||
      !disposition.decisionSource || !fs.existsSync(path.resolve(root, disposition.decisionSource)) || !disposition.frames?.length)
    errors.push('Retired descendant requires explicit IDs, a governing decision source and a reason');
  for (const id of disposition.frames ?? []) {
    if (!activeIds.has(id) || roots.has(id) || retiredFrames.has(id)) errors.push(`unknown/duplicate/non-descendant retirement ${id}`);
    activeIds.delete(id);
    supersededIds.add(id);
    retiredFrames.set(id, { disposition: disposition.disposition, reason: disposition.reason, decisionSource: disposition.decisionSource });
  }
}
for (const id of frameById.keys())
  if (activeIds.has(id) === supersededIds.has(id)) errors.push(`unclassified/ambiguous pen frame ${id}`);

const resolvedTargets = new Map();
for (const [id, target] of Object.entries(inventory.targets)) {
  const types = ['storyId', 'interactionId', 'served'].filter((key) => Object.hasOwn(target, key));
  if (types.length !== 1) { errors.push(`target ${id}: exactly one executable owner required`); continue; }
  let keys = [];
  if (target.storyId) {
    addStories([target.storyId], `target ${id}`);
    keys = target.widths.flatMap((width) => target.schemes.map((theme) => `${target.storyId}|${width}|${theme}`));
    for (const key of keys) if (!planByKey.has(key)) errors.push(`target ${id}: missing executable story capture ${key}`);
  } else if (target.interactionId) {
    keys = plan.filter((item) => item.interactionId === target.interactionId).map((item) => item.key);
    if (!keys.length) errors.push(`target ${id}: unknown/missing interaction ${target.interactionId}`);
  } else {
    const served = target.served;
    if (!served.keys?.length || !target.expectedState || target.referenceDisposition !== 'preserve-exact-served-comparator') errors.push(`target ${id}: explicit served keys, expected state and disposition required`);
    keys = (served.keys ?? []).map((key) => `${served.comparator}:${served.scope}:${served.kind}:${key}`);
    for (const key of keys) if (!servedCases.has(key)) errors.push(`target ${id}: unknown exact served capture ${key}`);
  }
  resolvedTargets.set(id, keys);
}
const frameBindings = new Map(), usedTargets = new Set();
for (const binding of inventory.bindings) {
  if (!binding.frames?.length || !binding.targets?.length || !binding.visibleOwnership?.trim()) errors.push('Every binding needs explicit frame IDs, capture targets and visible ownership');
  const keys = [];
  for (const target of binding.targets ?? []) {
    usedTargets.add(target);
    if (!resolvedTargets.has(target)) errors.push(`binding: unknown capture target ${target}`);
    keys.push(...(resolvedTargets.get(target) ?? []));
  }
  for (const id of binding.frames ?? []) {
    if (!activeIds.has(id)) errors.push(`binding claims unknown/inactive pen frame ${id}`);
    if (frameBindings.has(id)) errors.push(`duplicate executable pen binding ${id}`);
    frameBindings.set(id, { name: frameById.get(id)?.node.name, captures: keys, visibleOwnership: binding.visibleOwnership });
  }
}
for (const id of activeIds) if (!frameBindings.has(id)) errors.push(`active pen frame lacks an executable binding: ${id}`);
for (const id of resolvedTargets.keys()) if (!usedTargets.has(id)) errors.push(`unused capture target ${id}`);
for (const interaction of contract.interactionCaptures ?? []) for (const frame of interaction.frames ?? []) {
  const captures = frameBindings.get(frame)?.captures ?? [];
  if (!plan.some((item) => item.interactionId === interaction.id && captures.includes(item.key)))
    errors.push(`interaction ${interaction.id}: frame ${frame} has no matching executable binding`);
}
for (const id of availableStories) if (!mappedStories.has(id)) errors.push(`orphaned story ${id}`);
const approved = readJson(contract.referenceMap);
errors.push(...referenceErrors(approved, plan));

const report = {
  activeStories: [...mappedStories].sort(), servedChecks: [...mappedChecks].sort(),
  executableStoryCaptures: plan.map((item) => ({ ...item, referenceDisposition: 'pending-storybook-review' })),
  exactServedCaptures: [...servedCases.values()], visualAcceptances: [...acceptanceKeys],
  penFrames: { active: activeIds.size, superseded: supersededIds.size, executable: frameBindings.size, inherited: 0 },
  frameBindings: Object.fromEntries(frameBindings), errors,
  retiredFrames: Object.fromEntries(retiredFrames),
};
const out = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook');
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'contract-report.json'), JSON.stringify(report, null, 2) + '\n');
if (errors.length) { console.error(errors.join('\n')); process.exitCode = 1; }
else console.log(`Storybook contract: ${mappedStories.size} stories, ${plan.length} executable captures, ${servedCases.size} exact served states, ${frameBindings.size} explicit pen bindings; no inherited coverage.`);
