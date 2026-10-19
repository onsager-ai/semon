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
    .filter(([id, entry]) => entry.type === 'story' && id.startsWith(contract.ownedStoryPrefix))
    .map(([id]) => id),
);

for (const [checkId, relativeFile] of Object.entries(contract.servedChecks)) {
  if (!fs.existsSync(path.resolve(here, relativeFile)))
    errors.push(`Missing check file for ${checkId}: ${relativeFile}`);
}

for (const surface of contract.productionSurfaces) {
  addStories(surface.storyIds, `production surface ${surface.id}`);
  for (const checkId of surface.servedChecks ?? [])
    addCheck(checkId, `production surface ${surface.id}`);
  for (const module of surface.modules ?? []) {
    const ownerPath = path.resolve(root, 'ui/stories', module);
    const hasSourceFile = ['', '.ts', '.tsx', '.js', '.mjs'].some((extension) =>
      fs.existsSync(ownerPath + extension),
    );
    const baseName = path.basename(module);
    if (!hasSourceFile) {
      errors.push(`production surface ${surface.id}: missing production owner ${module}`);
    }
    const directlyImported =
      storySource.includes(`../src/lib/${baseName}`) ||
      storySource.includes(`../src/app/${baseName}`);
    const barrelImported =
      gallerySource.includes("from '../../src/lib'") &&
      libraryIndex.includes(`from './${baseName}'`);
    if (!directlyImported && !barrelImported) {
      errors.push(`production surface ${surface.id}: owner ${module} is not imported by a story`);
    }
  }
}

const interactionIds = new Set();
for (const interaction of contract.interactionCaptures ?? []) {
  if (!interaction.id || interactionIds.has(interaction.id)) {
    errors.push(
      `interaction capture has a missing or duplicate state ID: ${interaction.id ?? '(empty)'}`,
    );
  }
  interactionIds.add(interaction.id);
  addStories([interaction.storyId], `interaction state ${interaction.id}`);
  addCheck('visual-pixels', `interaction state ${interaction.id}`);
  if (!Array.isArray(interaction.widths) || interaction.widths.length === 0) {
    errors.push(
      `interaction state ${interaction.id}: at least one explicit viewport width is required`,
    );
  }
  if (!Array.isArray(interaction.schemes) || interaction.schemes.length === 0) {
    errors.push(
      `interaction state ${interaction.id}: at least one explicit color scheme is required`,
    );
  }
  if (!Array.isArray(interaction.actions) || interaction.actions.length === 0) {
    errors.push(`interaction state ${interaction.id}: an executable action sequence is required`);
  }
  if (!Array.isArray(interaction.assertions) || interaction.assertions.length === 0) {
    errors.push(
      `interaction state ${interaction.id}: at least one visible-state assertion is required`,
    );
  }
  for (const assertion of [
    ...(interaction.assertions ?? []),
    ...(interaction.actions ?? []).flatMap((action) => action.assertionsAfter ?? []),
    ...(interaction.postCaptureAssertions ?? []),
  ]) {
    if (!['visible', 'absent'].includes(assertion.kind)) {
      errors.push(
        `interaction state ${interaction.id}: unsupported assertion kind ${assertion.kind}`,
      );
    }
  }
  for (const width of interaction.widths ?? []) {
    if (!Number.isInteger(width) || width < 300 || width > 2560) {
      errors.push(`interaction state ${interaction.id}: invalid viewport width ${width}`);
    }
  }
  for (const scheme of interaction.schemes ?? []) {
    if (!contract.pixelPolicy.schemes.includes(scheme)) {
      errors.push(`interaction state ${interaction.id}: unknown color scheme ${scheme}`);
    }
  }
  for (const frameId of interaction.frames ?? []) {
    if (contract.pen.roots.some((root) => root.id === frameId && root.disposition === 'active'))
      continue;
    if ((contract.pen.descendantStories?.[frameId] ?? []).includes(interaction.storyId)) continue;
    errors.push(
      `interaction state ${interaction.id}: frame ${frameId} is not explicitly mapped to ${interaction.storyId}`,
    );
  }
}

const visualCoverage = readJson(path.resolve(here, contract.visualCoverage.source));
const visualFrames = new Map();
for (const frame of visualCoverage.frames) {
  const matches = contract.visualCoverage.storyRules.filter((rule) =>
    new RegExp(rule.pattern).test(frame),
  );
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
const supersededPenIds = new Set();
const declaredRoots = new Set(contract.pen.roots.map((entry) => entry.id));
const sourceRoots = new Set(
  (pen.children ?? []).filter((node) => node.type === 'frame').map((node) => node.id),
);
for (const id of sourceRoots)
  if (!declaredRoots.has(id)) errors.push(`unmapped top-level pen frame ${id}`);
for (const id of declaredRoots)
  if (!sourceRoots.has(id)) errors.push(`declared pen root ${id} is not top-level`);

for (const rootEntry of contract.pen.roots) {
  if (!['active', 'superseded'].includes(rootEntry.disposition)) {
    errors.push(`pen root ${rootEntry.id} needs an explicit active or superseded disposition`);
  }
  const rootFrame = frameById.get(rootEntry.id);
  if (!rootFrame) {
    errors.push(`pen root ${rootEntry.id} is missing from ${contract.pen.source}`);
    continue;
  }
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
  const isActive = rootEntry.disposition === 'active';
  const groups = isActive
    ? contract.pen.activeDescendantGroups
    : contract.pen.supersededDescendantGroups;
  const declaredDescendants = groups
    .filter((group) => group.root === rootEntry.id)
    .flatMap((group) => group.ids);
  if (
    new Set(declaredDescendants).size !== declaredDescendants.length ||
    [...new Set(declaredDescendants)].sort().join('\n') !== [...descendants].sort().join('\n')
  ) {
    errors.push(
      `pen root ${rootEntry.id}: ${rootEntry.disposition} descendant list is missing, duplicated, or contains an unknown frame`,
    );
  }
  if (groups.filter((item) => item.root === rootEntry.id).length !== 1) {
    errors.push(`pen root ${rootEntry.id}: expected exactly one complete descendant group`);
  }
  if (isActive) {
    addStories(rootEntry.storyIds, `pen root ${rootEntry.id}`);
    addCheck(contract.pen.servedCheck, `pen root ${rootEntry.id}`);
    activePenIds.add(rootEntry.id);
    for (const id of descendants) {
      activePenIds.add(id);
      activeRoots.push({ id, inheritedFrom: rootEntry.id });
    }
  } else {
    if (rootEntry.storyIds?.length)
      errors.push(`superseded pen root ${rootEntry.id} must not claim active Storybook owners`);
    if (!rootEntry.reason)
      errors.push(`superseded pen root ${rootEntry.id} needs a current-source rationale`);
    supersededPenIds.add(rootEntry.id);
    for (const id of descendants) supersededPenIds.add(id);
  }
}

for (const group of contract.pen.activeDescendantGroups) {
  if (!declaredRoots.has(group.root))
    errors.push(`active pen descendant group references unknown root ${group.root}`);
}
for (const group of contract.pen.supersededDescendantGroups) {
  if (!declaredRoots.has(group.root))
    errors.push(`superseded pen descendant group references unknown root ${group.root}`);
}
for (const id of frameById.keys()) {
  if (!activePenIds.has(id) && !supersededPenIds.has(id))
    errors.push(`unclassified pen frame ${id}`);
  if (activePenIds.has(id) && supersededPenIds.has(id))
    errors.push(`pen frame ${id} is both active and superseded`);
}
for (const [id, storyIds] of Object.entries(contract.pen.descendantStories ?? {})) {
  if (!activePenIds.has(id)) errors.push(`pen descendant story mapping ${id} is not active`);
  addStories(storyIds, `pen descendant ${id}`);
}

for (const storyId of mappedStories) {
  if (!availableStories.has(storyId)) errors.push(`unknown Storybook story ID ${storyId}`);
}
for (const storyId of availableStories) {
  if (!mappedStories.has(storyId))
    errors.push(`orphaned Storybook story ${storyId} has no active contract mapping`);
}

const report = {
  activeStories: [...mappedStories].sort(),
  servedChecks: [...mappedChecks].sort(),
  visualCoverage: { frames: visualFrames.size, ids: [...visualFrames.keys()].sort() },
  visualAcceptances: acceptanceKeys.size,
  galleryStates: gallery.states?.length ?? 0,
  penFrames: {
    active: activePenIds.size,
    superseded: supersededPenIds.size,
    inherited: activeRoots.length,
  },
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
