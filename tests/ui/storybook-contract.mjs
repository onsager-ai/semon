import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));
const root = path.resolve(here, '../..');
const uiStories = path.join(root, 'ui/stories/ProductionSurfaces.stories.tsx');
const galleryFixture = path.join(root, 'ui/tests/fixtures/design-gallery.tsx');
const storySource = fs.readFileSync(uiStories, 'utf8');
const gallerySource = fs.readFileSync(galleryFixture, 'utf8');
const libraryIndex = fs.readFileSync(path.join(root, 'ui/src/lib/index.ts'), 'utf8');
const contract = readJson(path.join(here, 'storybook-contract.json'));
const errors = [];
const mappedStories = new Set();
const mappedChecks = new Set();

function readJson(file) {
  return JSON.parse(fs.readFileSync(file, 'utf8'));
}

function addStories(storyIds, source) {
  if (!Array.isArray(storyIds) || storyIds.length === 0) {
    errors.push(`${source}: at least one Storybook owner is required`);
    return;
  }
  for (const storyId of storyIds) mappedStories.add(storyId);
}

function addCheck(checkId, source) {
  if (!Object.hasOwn(contract.servedChecks, checkId)) {
    errors.push(`${source}: unknown served check ${checkId}`);
    return;
  }
  mappedChecks.add(checkId);
}

const storyIndexPath = path.resolve(here, contract.storyIndex);
if (!fs.existsSync(storyIndexPath)) {
  throw new Error(`Missing ${contract.storyIndex}; run the Storybook static build first`);
}
const storyIndex = readJson(storyIndexPath);
const availableStories = new Set(
  Object.entries(storyIndex.entries ?? {})
    .filter(([, entry]) => entry.type === 'story')
    .map(([id]) => id),
);

for (const [checkId, relativeFile] of Object.entries(contract.servedChecks)) {
  if (!fs.existsSync(path.resolve(here, relativeFile))) errors.push(`Missing check file for ${checkId}: ${relativeFile}`);
}

for (const surface of contract.productionSurfaces) {
  addStories(surface.storyIds, `production surface ${surface.id}`);
  for (const checkId of surface.servedChecks ?? []) addCheck(checkId, `production surface ${surface.id}`);
  for (const module of surface.modules ?? []) {
    const ownerPath = path.resolve(root, 'ui/stories', module) + '.tsx';
    const baseName = path.basename(module);
    if (!fs.existsSync(ownerPath)) {
      errors.push(`production surface ${surface.id}: missing production owner ${module}`);
    }
    const directlyImported = storySource.includes(`../src/lib/${baseName}`) || storySource.includes(`../src/app/${baseName}`);
    const barrelImported =
      gallerySource.includes("from '../../src/lib'") &&
      libraryIndex.includes(`from './${baseName}'`);
    if (!directlyImported && !barrelImported) {
      errors.push(`production surface ${surface.id}: owner ${module} is not imported by a story`);
    }
  }
}

const visualCoverage = readJson(path.resolve(here, contract.visualCoverage.source));
const visualFrames = new Map();
for (const frame of visualCoverage.frames) {
  const matches = contract.visualCoverage.storyRules.filter((rule) => new RegExp(rule.pattern).test(frame));
  if (matches.length !== 1) {
    errors.push(`visual frame ${frame}: expected one story rule, found ${matches.length}`);
    continue;
  }
  const storyIds = matches[0].storyIds;
  addStories(storyIds, `visual frame ${frame}`);
  addCheck(contract.visualCoverage.servedCheck, `visual frame ${frame}`);
  visualFrames.set(frame, {
    storyIds,
    servedCheck: contract.visualCoverage.servedCheck,
  });
}

const acceptance = readJson(path.resolve(here, contract.visualAcceptance.source));
const acceptanceKeys = new Set();
for (const item of acceptance.acceptances ?? []) {
  const key = `${item.comparator}:${item.scope}:${item.key}`;
  if (acceptanceKeys.has(key)) errors.push(`duplicate visual acceptance key ${key}`);
  acceptanceKeys.add(key);
  const checkId = contract.visualAcceptance.comparatorChecks[item.comparator];
  if (!checkId) errors.push(`visual acceptance ${key}: comparator has no served check`);
  else addCheck(checkId, `visual acceptance ${key}`);
}

const gallery = readJson(path.resolve(here, contract.gallery.source));
for (const state of gallery.states ?? []) {
  addStories(contract.gallery.storyIds, `gallery state ${state}`);
  addCheck(contract.gallery.servedCheck, `gallery state ${state}`);
}

const pen = readJson(path.resolve(here, contract.pen.source));
const frameById = new Map();
function collectFrames(nodes, parent = null) {
  for (const node of nodes ?? []) {
    if (node.type === 'frame' && typeof node.id === 'string') {
      frameById.set(node.id, { node, parent });
      collectFrames(node.children, node.id);
    } else {
      collectFrames(node.children, parent);
    }
  }
}
collectFrames(pen.children);

const activePenIds = new Set();
const activeRoots = [];
for (const rootEntry of contract.pen.roots) {
  const rootFrame = frameById.get(rootEntry.id);
  if (!rootFrame) {
    errors.push(`pen root ${rootEntry.id} is missing from ${contract.pen.source}`);
    continue;
  }
  addStories(rootEntry.storyIds, `pen root ${rootEntry.id}`);
  addCheck(contract.pen.servedCheck, `pen root ${rootEntry.id}`);
  activePenIds.add(rootEntry.id);
  const descendants = [...frameById.entries()]
    .filter(([, value]) => {
      let parent = value.parent;
      while (parent) {
        if (parent === rootEntry.id) return true;
        parent = frameById.get(parent)?.parent ?? null;
      }
      return false;
    })
    .map(([id]) => id);
  for (const id of descendants) {
    if (contract.pen.supersededDescendants[id]) continue;
    activePenIds.add(id);
    activeRoots.push({ id, inheritedFrom: rootEntry.id });
  }
}

const supersededIds = Object.keys(contract.pen.supersededDescendants);
for (const id of supersededIds) {
  if (!frameById.has(id)) errors.push(`superseded pen descendant ${id} is missing from source`);
  if (activePenIds.has(id)) errors.push(`pen descendant ${id} is both active and superseded`);
}
for (const [id, storyIds] of Object.entries(contract.pen.descendantStories ?? {})) {
  if (!activePenIds.has(id)) errors.push(`pen descendant story mapping ${id} is not active`);
  addStories(storyIds, `pen descendant ${id}`);
}

for (const storyId of mappedStories) {
  if (!availableStories.has(storyId)) errors.push(`unknown Storybook story ID ${storyId}`);
}
for (const storyId of availableStories) {
  if (!mappedStories.has(storyId)) errors.push(`orphaned Storybook story ${storyId} has no active contract mapping`);
}

const report = {
  activeStories: [...mappedStories].sort(),
  servedChecks: [...mappedChecks].sort(),
  visualCoverage: { frames: visualFrames.size, ids: [...visualFrames.keys()].sort() },
  visualAcceptances: acceptanceKeys.size,
  galleryStates: gallery.states?.length ?? 0,
  penFrames: { active: activePenIds.size, superseded: supersededIds.length, inherited: activeRoots.length },
  errors,
};
const out = path.resolve(process.env.SEMON_UI_OUT ?? path.join(here, 'out'), 'storybook');
fs.mkdirSync(out, { recursive: true });
fs.writeFileSync(path.join(out, 'contract-report.json'), JSON.stringify(report, null, 2) + '\n');
if (errors.length) {
  console.error(errors.join('\n'));
  process.exitCode = 1;
} else {
  console.log(
    `Storybook contract: ${mappedStories.size} stories, ${visualFrames.size} visual frames, ${acceptanceKeys.size} exact visual acceptance records, ${activePenIds.size} active pen frames; ${mappedChecks.size} served checks.`,
  );
}
