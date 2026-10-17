// CI-only inspection of six open Filter screenshots from pinned old/new artifacts.
// It reads bounded extracted files; it does not capture or rewrite screenshots.
import { createHash } from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { MAX_RATIO, THRESHOLD } from './preservation-contract.mjs';

const KEYS = [
  '390-light-sessions-filter-open',
  '390-dark-sessions-filter-open',
  '820-light-sessions-filter-open',
  '820-dark-sessions-filter-open',
  '1280-light-sessions-filter-open',
  '1280-dark-sessions-filter-open',
];
const ROLES = ['candidate', 'reference', 'diff'];
const MAX_PNG_BYTES = 16 * 1024 * 1024;
const MAX_REPORT_BYTES = 5 * 1024 * 1024;
const MAX_JSON_ARTIFACT_BYTES = 1024 * 1024;
const MAX_IMAGE_PIXELS = 20_000_000;
const MAX_PNG_CHUNKS = 128;
const EXPECTED_PROVENANCE = {
  old: {
    candidate: '48d26b98956f3763d5d0933aa35197854be5f45c',
    candidateFingerprint: 'f8bac26d1c7736b3f72e19e6cbf9c0a818dc03c5e96ad7ddc09790d76848ca6a',
  },
  new: {
    candidate: '1ea3958c5c9a0ad67baec5448749d113f0e19595',
    candidateFingerprint: 'f8bac26d1c7736b3f72e19e6cbf9c0a818dc03c5e96ad7ddc09790d76848ca6a',
  },
  reference: '263a02da278716ab2309b8e34a463419e5c781e5',
  referenceHub: '503fbe090749ea1d58b9ca9fa4ccae7289ee426e',
  threshold: 0.1,
  maxRatio: 0.0001,
  mode: 'required',
  caseCount: 68,
};
const EXPECTED = {
  '390-light-sessions-filter-open': {
    old: {
      candidate: 'b7e72943117c01d8dc3b57b117483df4385dfbdeda9bd4f21da9fb2608da98da',
      reference: '3d1880cccc6cb4fdf81aa8f075d6357762bf4881b3d813179e1d15994eb65768',
      diff: 'f2da2af4f37041b2e384b45f45a8a4d052ac16159a28c69ee8473db56ba6ff62',
    },
    new: {
      candidate: 'b7e72943117c01d8dc3b57b117483df4385dfbdeda9bd4f21da9fb2608da98da',
      reference: '3d1880cccc6cb4fdf81aa8f075d6357762bf4881b3d813179e1d15994eb65768',
      diff: 'f2da2af4f37041b2e384b45f45a8a4d052ac16159a28c69ee8473db56ba6ff62',
    },
    strict: { pixels: 161019, ratio: 0.12229538826102807, size: null },
  },
  '390-dark-sessions-filter-open': {
    old: {
      candidate: '356819398952c78f3ea9bda8ec2222bae5988abb5a54dc4d7e046690a802c304',
      reference: 'a2491af93b7c921983b7360b11fba38378ef03b0fcd7aea49148a9ed39ae5e15',
      diff: 'c6769f2e764e9a737752421ec077e60d7146a366445f0944b1c48ba3475ce03f',
    },
    new: {
      candidate: '356819398952c78f3ea9bda8ec2222bae5988abb5a54dc4d7e046690a802c304',
      reference: 'a2491af93b7c921983b7360b11fba38378ef03b0fcd7aea49148a9ed39ae5e15',
      diff: 'c6769f2e764e9a737752421ec077e60d7146a366445f0944b1c48ba3475ce03f',
    },
    strict: { pixels: 24904, ratio: 0.018914813464576498, size: null },
  },
  '820-light-sessions-filter-open': {
    old: {
      candidate: '8c2542c2739f03776033bcc21e46b9cbb36cf64c2b24e381b502dc6c412b7575',
      reference: 'c344eea599ad2e93cbba140a439adf5d6deb49cf71d6295556fe1626aa1e665b',
      diff: '2816d86abb7c9879ca29b33b2df1376637fb713d7abf4557d1cd4a281a540d5d',
    },
    new: {
      candidate: 'cf2ed7b5f37c46e39ca3e807e6a4bd1f9152efc23fe3d6b6738e4abf6a952b59',
      reference: 'c344eea599ad2e93cbba140a439adf5d6deb49cf71d6295556fe1626aa1e665b',
      diff: '2816d86abb7c9879ca29b33b2df1376637fb713d7abf4557d1cd4a281a540d5d',
    },
    strict: { pixels: 54575, ratio: 0.07738939307997732, size: null },
  },
  '820-dark-sessions-filter-open': {
    old: {
      candidate: '343bfcfe1a18996d19faa55dc64518d8cdc26d97d04c7c979ef17e3de125eaeb',
      reference: 'c62d60f647d1a77462e37acf24aa11ec939be2de95397e63ed6aad1236a31fcd',
      diff: 'a8c83ef5b6a3abfb3742e5e508d9df222d2b64eae50461fdd1f18b090b3cf5bf',
    },
    new: {
      candidate: '27e0ab7798337c0b6bfb5ce4cce2cc8ab623bedc828e569c39721f8efb5b3a46',
      reference: 'c62d60f647d1a77462e37acf24aa11ec939be2de95397e63ed6aad1236a31fcd',
      diff: 'a8c83ef5b6a3abfb3742e5e508d9df222d2b64eae50461fdd1f18b090b3cf5bf',
    },
    strict: { pixels: 17827, ratio: 0.0252793533749291, size: null },
  },
  '1280-light-sessions-filter-open': {
    old: {
      candidate: '9e2bf70069d7271dedacab74fa1d0a25bcd1401e212e6c97199a74827ecf5af0',
      reference: 'caf9b40c3d7cc16b2b4139ef7957fd4a6cc103c91d4faacf5f6713640ad4f690',
      diff: '6394fe7dc55c4f3a0aabd0590b058dd94f8b587e2ce49478ead1f9bf85875769',
    },
    new: {
      candidate: 'bac8e33fb4a0b065d935e832e34117647dc73162e969075d534085b50991da2f',
      reference: 'caf9b40c3d7cc16b2b4139ef7957fd4a6cc103c91d4faacf5f6713640ad4f690',
      diff: '62ebd78313f89cfd97be03e821fba0bbc77d0c9ab100aeab5ebb8922fc5c0a0f',
    },
    strict: { pixels: 54617, ratio: 0.04961573401162791, size: null },
  },
  '1280-dark-sessions-filter-open': {
    old: {
      candidate: 'c2aa2d145ea709e08ce1e4253d8dfa7f0b1c0e31e2bc9ea99881e027b4870517',
      reference: 'badedb07ab54ff42d7475414e4e47d25b762c4e64b54bac5fb82288aa0d4f15c',
      diff: '14b2165c2ff56ed3353dfbdd955353fba5ffa239f71a7bec3ac1da65eb9a4ec8',
    },
    new: {
      candidate: '30daa47db683b1411a068bda227b5eddfd8c61746fc687616226051bd2c67bc0',
      reference: '025ba3b3385d0623242558ff2ace17294f06ff7f2a84f7f927b392604d92e514',
      diff: '14b2165c2ff56ed3353dfbdd955353fba5ffa239f71a7bec3ac1da65eb9a4ec8',
    },
    strict: { pixels: 17670, ratio: 0.016051962209302325, size: null },
  },
};
const SAMPLE_POINTS = [
  { name: 'top-left', x: 0, y: 0 },
  { name: 'top-center', x: 0.5, y: 0 },
  { name: 'top-right', x: 1, y: 0 },
  { name: 'left-center', x: 0, y: 0.5 },
  { name: 'center', x: 0.5, y: 0.5 },
  { name: 'right-center', x: 1, y: 0.5 },
  { name: 'bottom-left', x: 0, y: 1 },
  { name: 'bottom-center', x: 0.5, y: 1 },
  { name: 'bottom-right', x: 1, y: 1 },
];

const report = {
  diagnostic: 'filter-png-artifact-equivalence',
  keys: [],
  provenance: null,
  evidence: [],
  errors: [],
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  expectedThreshold: 0.1,
  expectedMaxRatio: 0.0001,
};
const oldDir = process.env.OLD_DIR ? path.resolve(process.env.OLD_DIR) : null;
const newDir = process.env.NEW_DIR ? path.resolve(process.env.NEW_DIR) : null;
const reportPath = path.resolve(
  process.env.REPORT_PATH ?? 'tests/ui/out/filter-png-artifact-diagnostic/report.json',
);

function sha256(bytes) {
  return createHash('sha256').update(bytes).digest('hex');
}

function readBoundedFile(file, limit, label) {
  const stat = fs.statSync(file);
  if (stat.size > limit) throw new Error(`${label} exceeds its ${limit}-byte bound`);
  const bytes = fs.readFileSync(file);
  return { bytes, sha256: sha256(bytes) };
}

function readBoundedJson(file, limit, label) {
  const parsed = readBoundedFile(file, limit, label);
  return { ...parsed, value: JSON.parse(parsed.bytes.toString('utf8')) };
}

function loadRunReport(directory, side) {
  try {
    const parsed = readBoundedJson(
      path.join(directory, 'report.json'),
      MAX_REPORT_BYTES,
      `${side} report`,
    );
    const source = parsed.value;
    if (!source || typeof source !== 'object' || Array.isArray(source))
      throw new Error('report root must be an object');
    const metadata = {
      candidate: source.candidate ?? null,
      reference: source.reference ?? null,
      referenceHub: source.referenceHub ?? null,
      candidateFingerprint: source.candidateFingerprint ?? null,
      threshold: source.threshold ?? null,
      maxRatio: source.maxRatio ?? null,
      mode: source.mode ?? null,
      caseCount: Array.isArray(source.results) ? source.results.length : null,
      errorCount: Array.isArray(source.errors) ? source.errors.length : null,
      reportSha256: parsed.sha256,
    };
    const expected = EXPECTED_PROVENANCE[side];
    for (const [field, value] of Object.entries(expected))
      if (metadata[field] !== value)
        report.errors.push(`${side} report provenance ${field} differs from its pinned value`);
    for (const field of ['reference', 'referenceHub', 'threshold', 'maxRatio', 'mode', 'caseCount'])
      if (metadata[field] !== EXPECTED_PROVENANCE[field])
        report.errors.push(
          `${side} report provenance ${field} differs from the comparison context`,
        );
    if (!Array.isArray(source.results))
      report.errors.push(`${side} report results are not an array`);
    if (metadata.errorCount !== 0)
      report.errors.push(`${side} preservation report has ${String(metadata.errorCount)} errors`);
    return { metadata, source };
  } catch (error) {
    report.errors.push(
      `${side} report provenance: ${String(error?.message ?? error).slice(0, 300)}`,
    );
    return null;
  }
}

function resultFor(run, key, side) {
  if (!run || !Array.isArray(run.source.results)) return null;
  const matches = run.source.results.filter((entry) => entry?.key === key);
  if (matches.length !== 1) {
    report.errors.push(`${side} report has ${matches.length} rows for ${key}; expected one`);
    return null;
  }
  return matches[0];
}

function validateComparisonEvidence(value, key, result, side) {
  const expected = {
    key,
    pixels: EXPECTED[key].strict.pixels,
    ratio: EXPECTED[key].strict.ratio,
    size: EXPECTED[key].strict.size,
    threshold: THRESHOLD,
    maxRatio: MAX_RATIO,
  };
  for (const [field, pinned] of Object.entries(expected))
    if (value?.[field] !== pinned)
      report.errors.push(
        `${side} ${key} comparison evidence ${field} differs from the pinned result`,
      );
  for (const field of ['key', 'pixels', 'ratio', 'size'])
    if (result?.[field] !== expected[field])
      report.errors.push(`${side} ${key} report ${field} differs from comparison evidence`);
}

function inspectEvidence(key, oldRun, newRun) {
  const evidence = {};
  for (const [side, run, directory] of [
    ['old', oldRun, oldDir],
    ['new', newRun, newDir],
  ]) {
    const result = resultFor(run, key, side);
    const files = {
      candidate: `${key}-candidate.json`,
      reference: `${key}-reference.json`,
      comparison: `${key}-comparison.json`,
    };
    const reportDigests = {
      candidate: result?.artifactDigests?.candidate?.evidence,
      reference: result?.artifactDigests?.reference?.evidence,
      comparison: result?.artifactDigests?.diff?.evidence,
    };
    evidence[side] = {};
    for (const role of Object.keys(files))
      try {
        const file = path.join(directory, files[role]);
        const label = `${side} ${key} ${role} evidence`;
        const parsed =
          role === 'comparison'
            ? readBoundedJson(file, MAX_JSON_ARTIFACT_BYTES, label)
            : readBoundedFile(file, MAX_JSON_ARTIFACT_BYTES, label);
        const expectedDigest = reportDigests[role];
        const digestMatches =
          typeof expectedDigest === 'string' && parsed.sha256 === expectedDigest;
        if (!digestMatches)
          report.errors.push(`${side} ${key} ${role} JSON hash differs from its report digest`);
        if (role === 'comparison') validateComparisonEvidence(parsed.value, key, result, side);
        evidence[side][role] = { sha256: parsed.sha256, reportDigestMatches: digestMatches };
      } catch (error) {
        report.errors.push(
          `${side} ${key} ${role} evidence: ${String(error?.message ?? error).slice(0, 300)}`,
        );
        evidence[side][role] = null;
      }
    if (result)
      for (const field of ['pixels', 'ratio', 'size'])
        if (result[field] !== EXPECTED[key].strict[field])
          report.errors.push(
            `${side} ${key} report ${field} differs from the pinned strict result`,
          );
  }
  const equal = {};
  for (const role of ['candidate', 'reference', 'comparison']) {
    const oldHash = evidence.old?.[role]?.sha256;
    const newHash = evidence.new?.[role]?.sha256;
    equal[role] = typeof oldHash === 'string' && oldHash === newHash;
    if (!equal[role]) report.errors.push(`${key} ${role} evidence changed between artifacts`);
  }
  report.evidence.push({ key, old: evidence.old, new: evidence.new, equal });
}

function chunkMetadata(bytes) {
  const signature = Buffer.from([137, 80, 78, 71, 13, 10, 26, 10]);
  if (bytes.length < 8 || !bytes.subarray(0, 8).equals(signature))
    throw new Error('PNG signature is invalid');
  const chunks = [];
  let offset = 8;
  let sawEnd = false;
  let ihdr = null;
  while (offset < bytes.length) {
    if (chunks.length >= MAX_PNG_CHUNKS) throw new Error('PNG chunk count exceeds bound');
    if (offset + 12 > bytes.length) throw new Error('truncated PNG chunk header');
    const length = bytes.readUInt32BE(offset);
    const type = bytes.toString('ascii', offset + 4, offset + 8);
    const end = offset + 12 + length;
    if (!/^[A-Za-z]{4}$/.test(type) || end > bytes.length)
      throw new Error('invalid or truncated PNG chunk');
    if (type === 'IHDR') {
      if (length !== 13 || ihdr) throw new Error('PNG IHDR is missing, duplicated or malformed');
      ihdr = {
        width: bytes.readUInt32BE(offset + 8),
        height: bytes.readUInt32BE(offset + 12),
        bitDepth: bytes[offset + 16],
        colorType: bytes[offset + 17],
        compression: bytes[offset + 18],
        filter: bytes[offset + 19],
        interlace: bytes[offset + 20],
      };
    }
    chunks.push({
      type,
      length,
      crc: bytes
        .readUInt32BE(offset + 8 + length)
        .toString(16)
        .padStart(8, '0'),
    });
    offset = end;
    if (type === 'IEND') {
      if (length !== 0 || offset !== bytes.length) throw new Error('invalid or trailing PNG data');
      sawEnd = true;
      break;
    }
  }
  if (!sawEnd || !ihdr) throw new Error('PNG is missing IHDR or IEND');
  if (!ihdr.width || !ihdr.height || ihdr.width * ihdr.height > MAX_IMAGE_PIXELS)
    throw new Error('PNG dimensions exceed diagnostic bounds');
  return { ihdr, chunks };
}

function loadPng(root, key, role, side) {
  const expectedHash = EXPECTED[key][side][role];
  const file = path.join(root, `${key}-${role}.png`);
  try {
    const stat = fs.statSync(file);
    if (!stat.isFile() || stat.size > MAX_PNG_BYTES)
      throw new Error(`PNG is not a bounded regular file (${stat.size} bytes)`);
    const bytes = fs.readFileSync(file);
    const digest = sha256(bytes);
    if (digest !== expectedHash)
      report.errors.push(`${side} ${key} ${role} byte hash differs from pinned manifest`);
    const metadata = chunkMetadata(bytes);
    const image = PNG.sync.read(bytes);
    return {
      digest,
      expectedHash,
      metadata,
      image,
      bytes: bytes.length,
    };
  } catch (error) {
    report.errors.push(`${side} ${key} ${role}: ${String(error?.message ?? error).slice(0, 300)}`);
    return null;
  }
}

function rgbaChanges(oldImage, newImage) {
  const width = Math.min(oldImage.width, newImage.width);
  const height = Math.min(oldImage.height, newImage.height);
  const channelNames = ['r', 'g', 'b', 'a'];
  const channels = { r: 0, g: 0, b: 0, a: 0 };
  const maxChannelDelta = { r: 0, g: 0, b: 0, a: 0 };
  const firstChangedSamples = [];
  let changedPixels = 0;
  let minX = width,
    minY = height,
    maxX = -1,
    maxY = -1;
  for (let y = 0; y < height; y++)
    for (let x = 0; x < width; x++) {
      const oldAt = (y * oldImage.width + x) * 4;
      const newAt = (y * newImage.width + x) * 4;
      let changed = false;
      for (let channel = 0; channel < 4; channel++)
        if (oldImage.data[oldAt + channel] !== newImage.data[newAt + channel]) {
          channels[channelNames[channel]]++;
          changed = true;
        }
      const delta = channelNames.map((name, channel) => {
        const amount = Math.abs(oldImage.data[oldAt + channel] - newImage.data[newAt + channel]);
        maxChannelDelta[name] = Math.max(maxChannelDelta[name], amount);
        return amount;
      });
      if (changed) {
        changedPixels++;
        minX = Math.min(minX, x);
        minY = Math.min(minY, y);
        maxX = Math.max(maxX, x);
        maxY = Math.max(maxY, y);
        if (firstChangedSamples.length < 8)
          firstChangedSamples.push({
            x,
            y,
            oldRGBA: [...oldImage.data.subarray(oldAt, oldAt + 4)],
            newRGBA: [...newImage.data.subarray(newAt, newAt + 4)],
            absoluteDeltaRGBA: delta,
          });
      }
    }
  return {
    decodedPixelEquality:
      oldImage.width === newImage.width &&
      oldImage.height === newImage.height &&
      oldImage.data.equals(newImage.data),
    changedPixels,
    changedChannels: channels,
    maxAbsoluteChannelDelta: maxChannelDelta,
    firstChangedSamples,
    bounds: maxX < 0 ? null : [minX, minY, maxX - minX + 1, maxY - minY + 1],
    comparedSize: [width, height],
    sizeChanged: oldImage.width !== newImage.width || oldImage.height !== newImage.height,
    oldSize: [oldImage.width, oldImage.height],
    newSize: [newImage.width, newImage.height],
  };
}

function colorAt(image, xFraction, yFraction) {
  const x = Math.round(xFraction * (image.width - 1));
  const y = Math.round(yFraction * (image.height - 1));
  const at = (y * image.width + x) * 4;
  return { x, y, rgba: [...image.data.subarray(at, at + 4)] };
}

function strictComparison(candidate, reference, expected, side, key) {
  const result = compare(candidate, reference, THRESHOLD);
  const actual = { pixels: result.pixels, ratio: result.ratio, size: result.size };
  const matchesExpected =
    actual.pixels === expected.pixels &&
    actual.ratio === expected.ratio &&
    JSON.stringify(actual.size) === JSON.stringify(expected.size);
  if (!matchesExpected)
    report.errors.push(`${side} ${key} strict pixelmatch metrics differ from pinned manifest`);
  return {
    ...actual,
    threshold: THRESHOLD,
    maxRatio: MAX_RATIO,
    withinExistingGate: actual.size === null && actual.ratio <= MAX_RATIO,
    expected,
    matchesExpected,
    difference: result.difference,
  };
}

function inspectKey(key) {
  const assetRows = [];
  const images = { old: {}, new: {} };
  for (const side of ['old', 'new'])
    for (const role of ROLES) {
      const loaded = loadPng(side === 'old' ? oldDir : newDir, key, role, side);
      if (loaded) {
        images[side][role] = loaded.image;
        assetRows.push({
          side,
          role,
          sha256: loaded.digest,
          expectedSha256: loaded.expectedHash,
          byteHashMatches: loaded.digest === loaded.expectedHash,
          bytes: loaded.bytes,
          png: loaded.metadata,
        });
      }
    }
  const pixelRows = [];
  for (const role of ROLES) {
    const oldImage = images.old[role];
    const newImage = images.new[role];
    if (!oldImage || !newImage) continue;
    const changes = rgbaChanges(oldImage, newImage);
    const strict = compare(oldImage, newImage, THRESHOLD);
    const samples = SAMPLE_POINTS.map((point) => ({
      name: point.name,
      old: colorAt(oldImage, point.x, point.y),
      new: colorAt(newImage, point.x, point.y),
    }));
    if (!changes.decodedPixelEquality)
      report.errors.push(`old/new ${key} ${role} decoded pixels differ`);
    pixelRows.push({
      role,
      ...changes,
      existingPixelmatch: {
        pixels: strict.pixels,
        ratio: strict.ratio,
        size: strict.size,
        threshold: THRESHOLD,
      },
      samples,
    });
  }
  const strictOld =
    images.old.candidate && images.old.reference
      ? strictComparison(
          images.old.candidate,
          images.old.reference,
          EXPECTED[key].strict,
          'old',
          key,
        )
      : null;
  const strictNew =
    images.new.candidate && images.new.reference
      ? strictComparison(
          images.new.candidate,
          images.new.reference,
          EXPECTED[key].strict,
          'new',
          key,
        )
      : null;
  const strictUnchanged =
    !!strictOld &&
    !!strictNew &&
    strictOld.pixels === strictNew.pixels &&
    strictOld.ratio === strictNew.ratio &&
    JSON.stringify(strictOld.size) === JSON.stringify(strictNew.size);
  if (strictOld && strictNew && !strictUnchanged)
    report.errors.push(`old/new ${key} existing strict candidate/reference comparison changed`);
  return {
    key,
    assets: assetRows,
    oldStrict: strictOld,
    newStrict: strictNew,
    strictUnchanged,
    pixels: pixelRows,
  };
}

try {
  if (!oldDir || !newDir)
    throw new Error('OLD_DIR and NEW_DIR must name extracted preservation directories');
  if (THRESHOLD !== report.expectedThreshold || MAX_RATIO !== report.expectedMaxRatio)
    report.errors.push('existing preservation threshold or max ratio differs from pinned values');
  if (!fs.statSync(oldDir).isDirectory() || !fs.statSync(newDir).isDirectory())
    throw new Error('OLD_DIR and NEW_DIR must be directories');
  const oldRun = loadRunReport(oldDir, 'old');
  const newRun = loadRunReport(newDir, 'new');
  const sharedFields = [
    'reference',
    'referenceHub',
    'candidateFingerprint',
    'threshold',
    'maxRatio',
    'mode',
    'caseCount',
  ];
  const sharedContextMatches =
    !!oldRun &&
    !!newRun &&
    sharedFields.every((field) => oldRun.metadata[field] === newRun.metadata[field]);
  if (!sharedContextMatches)
    report.errors.push(
      'old/new report provenance does not describe the same pinned capture context',
    );
  report.provenance = {
    old: oldRun?.metadata ?? null,
    new: newRun?.metadata ?? null,
    sharedContextMatches,
  };
  for (const key of KEYS)
    try {
      report.keys.push(inspectKey(key));
    } catch (error) {
      report.errors.push(`${key} inspection: ${String(error?.message ?? error).slice(0, 300)}`);
      report.keys.push({
        key,
        assets: [],
        oldStrict: null,
        newStrict: null,
        strictUnchanged: false,
        pixels: [],
      });
    }
  for (const key of KEYS) inspectEvidence(key, oldRun, newRun);
} catch (error) {
  report.errors.push(String(error?.message ?? error).slice(0, 500));
} finally {
  fs.mkdirSync(path.dirname(reportPath), { recursive: true });
  fs.writeFileSync(reportPath, JSON.stringify(report, null, 2) + '\n');
}

console.log(
  JSON.stringify({
    diagnostic: report.diagnostic,
    keys: report.keys.length,
    decodedPixelPairs: report.keys.reduce((count, row) => count + row.pixels.length, 0),
    strictUnchanged: report.keys.filter((row) => row.strictUnchanged).length,
    errors: report.errors.length,
    report: reportPath,
  }),
);
if (report.errors.length) process.exitCode = 1;
