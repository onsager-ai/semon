// Compare the served application with the pinned complete application, or
// compare two served copies for pushed-data parity. Historical frame names
// and regions remain a coverage contract; the mockup renderers are not loaded.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import { ENV, launch, served, goto, data, titleOf, waitForCaptureReady } from './lib.mjs';
import { sample, BASE } from './fixture.mjs';
import { compareDomEvidence, recordDomDiffSummary } from './dom-evidence.mjs';
import {
  assessPixelComparison,
  assessVisualAcceptances,
  expectedPixelCoverage,
  PARITY_MODEL_SOURCE,
  PIXEL_REGIONS,
  REFERENCE,
  REFERENCE_MODEL_SOURCE,
  sha256,
  THRESHOLD,
  MAX_RATIO,
} from './preservation-contract.mjs';

const scope = process.env.SEMON_PIXEL_SCOPE ?? 'main';
const coverage = expectedPixelCoverage(scope);
const expectedFrames = [...new Set(coverage.frames.map((key) => key.split('|')[1]))];
const selectedBase = (process.env.SEMON_PRESERVATION_BASE ?? '').replace(/\/$/, ''),
  selectedToken = process.env.SEMON_PRESERVATION_TOKEN ?? '',
  parityBase = (process.env.SEMON_REF_BASE ?? '').replace(/\/$/, ''),
  parityToken = process.env.SEMON_REF_TOKEN ?? '',
  selfParity = Boolean(parityBase);
if (Boolean(selectedBase) === selfParity)
  throw new Error('Set exactly one of the selected complete reference or served parity pair');
if ((selectedBase && !selectedToken) || (parityBase && !parityToken))
  throw new Error('The reference server token is missing');

const OUT = path.join(ENV.out, 'pixels');
fs.mkdirSync(OUT, { recursive: true });
const report = {
  candidate: process.env.GITHUB_SHA ?? null,
  candidateFingerprint: null,
  reference: selfParity ? 'served-parity' : REFERENCE.source,
  ...(selfParity ? {} : { referenceHub: REFERENCE.hub }),
  referenceModelSource: selfParity ? PARITY_MODEL_SOURCE : REFERENCE_MODEL_SOURCE,
  referenceModelDigest: null,
  scope,
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  coverageSource: 'tests/ui/visual-coverage.json',
  results: [],
  regions: [],
  errors: [],
  apiDiagnostics: [],
  routeMappings: [],
  domDiffSummary: { casesCompared: 0, casesWithChanges: 0, bySelector: {} },
};
let browser;

function fileKey(key) {
  return key.replace(/[^a-zA-Z0-9_-]+/g, '_');
}
function save(key, suffix, bytes) {
  fs.writeFileSync(path.join(OUT, fileKey(key) + '-' + suffix + '.png'), bytes);
}
function saveJson(key, suffix, value) {
  const bytes = Buffer.from(JSON.stringify(value, null, 2) + '\n');
  fs.writeFileSync(path.join(OUT, fileKey(key) + '-' + suffix + '.json'), bytes);
  return sha256(bytes);
}

async function snapshot(page) {
  return page.evaluate(() => {
    const selectors = [
      '#sidebar',
      '#topbar',
      '#lanes',
      '#page',
      '.nrow',
      '.think',
      '.tsum',
      '.session-dock',
      'dialog[open]',
    ];
    return {
      historyState: history.state,
      pathname: location.pathname,
      selectors: Object.fromEntries(
        selectors.map((selector) => [
          selector,
          [...document.querySelectorAll(selector)].slice(0, 60).map((node) => {
            const box = node.getBoundingClientRect(),
              css = getComputedStyle(node);
            return {
              text: node.textContent.slice(0, 2000),
              box: [box.x, box.y, box.width, box.height],
              font: css.font,
              color: css.color,
              background: css.backgroundColor,
              padding: css.padding,
              gap: css.gap,
              icons: [...node.querySelectorAll('img')]
                .slice(0, 40)
                .map((image) => image.getAttribute('src')),
              paths: [...node.querySelectorAll('path')]
                .slice(0, 80)
                .map((item) => item.getAttribute('d')),
            };
          }),
        ]),
      ),
    };
  });
}

function minute(t) {
  return Math.round(((t - BASE) / 60000) * 1000) / 1000;
}
async function pinnedReferenceModel(base, token) {
  const origin = new URL(base).origin,
    landing = await fetch(base + '/?t=' + encodeURIComponent(token));
  if (!landing.ok || new URL(landing.url).origin !== origin)
    throw new Error(`pinned reference model landing failed: HTTP ${landing.status}`);
  const cookies =
      typeof landing.headers.getSetCookie === 'function'
        ? landing.headers.getSetCookie()
        : [landing.headers.get('set-cookie')].filter(Boolean),
    cookie = cookies.map((value) => value.split(';')[0]).join('; ');
  if (!cookie)
    throw new Error('pinned reference model landing did not set an authorization cookie');
  const response = await fetch(base + '/api/model?delta=1&content=1', {
    headers: { cookie, accept: 'application/json' },
  });
  if (!response.ok || new URL(response.url).origin !== origin)
    throw new Error(`pinned reference model request failed: HTTP ${response.status}`);
  const bytes = Buffer.from(await response.arrayBuffer()),
    model = JSON.parse(bytes.toString('utf8'));
  if (!Array.isArray(model.handoffs) || !Array.isArray(model.turns))
    throw new Error('pinned reference model is missing handoffs or turns');
  return { model, digest: sha256(bytes) };
}

function referenceHandoffs(D, source, referenceModel) {
  const ids = new Map();
  for (const h of source.H) {
    const match = D.H.find(
      (candidate) =>
        !ids.has(candidate.id) &&
        candidate.kind === h.kind &&
        candidate.from === h.from &&
        candidate.to === h.to &&
        minute(candidate.at) === h.at,
    );
    if (!match) continue;
    if (!referenceModel) {
      ids.set(match.id, { fixtureId: h.id, modelId: h.id });
      continue;
    }
    const semantic = (candidate) =>
        candidate.kind === h.kind &&
        candidate.from === h.from &&
        candidate.to === h.to &&
        minute(candidate.at) === h.at,
      exact = referenceModel.handoffs.find((candidate) => candidate.id === h.id),
      matches = exact && semantic(exact) ? [exact] : referenceModel.handoffs.filter(semantic);
    if (matches.length !== 1) {
      report.errors.push(
        `Reference handoff mapping for ${match.id} -> ${h.id} has ${matches.length} exact model matches`,
      );
      continue;
    }
    ids.set(match.id, { fixtureId: h.id, modelId: matches[0].id });
  }
  return ids;
}

function originalTurnId(turn, referenceModel, handoffIds) {
  if (!referenceModel) return { id: turn.id };
  const start = turn.start ? handoffIds.get(turn.start)?.modelId : null,
    userText = typeof turn.text === 'string' ? turn.text : turn.u?.text,
    matches = referenceModel.turns.filter((candidate) => {
      if (candidate.sid !== turn.sid) return false;
      if (start) return candidate.start === start;
      if (!turn.u || !candidate.u) return false;
      return (
        (userText == null || candidate.text === userText) &&
        (turn.at == null || candidate.at === turn.at)
      );
    });
  if (matches.length !== 1)
    throw new Error(
      `pinned reference turn mapping for ${turn.sid}:${turn.id} has ${matches.length} exact ` +
        `matches (start=${start ?? 'none'}, user=${Boolean(turn.u)}, at=${turn.at ?? 'none'})`,
    );
  return matches[0];
}

function inputEvidence(turn) {
  if (!turn?.u) return null;
  const text = typeof turn.text === 'string' ? turn.text : (turn.u?.text ?? '');
  return { text: String(text).slice(0, 160), chars: String(text).length, at: turn.at ?? null };
}

function handoffEvidence(handoff, id) {
  return handoff
    ? {
        id: handoff.id,
        kind: handoff.kind,
        from: handoff.from,
        to: handoff.to,
        at: handoff.at,
      }
    : id
      ? { id }
      : null;
}

function screens(D, source, handoffIds, referenceModel) {
  const list = [
    { name: 'home', route: { v: 'home' } },
    { name: 'analytics', route: { v: 'analytics' } },
    { name: 'sessions', route: { v: 'sessions' } },
    { name: 'machines', route: { v: 'machines' } },
  ];
  for (const machine of Object.keys(D.MACHINE))
    list.push({ name: 'machine-' + machine, route: { v: 'machine', id: machine } });
  for (const sid of Object.keys(D.SESS))
    list.push({ name: 'session-' + sid, route: { v: 'session', id: sid } });
  for (const turn of D.turns) {
    if (!turn.sent?.length) continue;
    const at = (D.TX[turn.sid] ?? []).findIndex((entry) => entry.turn === turn.id);
    if (at < 0) {
      list.push({
        name: 'trace-' + turn.sid + '-' + String(turn.id).replace(/[^\w-]/g, '_'),
        route: { v: 'trace', sid: turn.sid, turn: turn.id },
        referenceRoute: null,
      });
      continue;
    }
    const fixtureTurn = turn.start
      ? handoffIds.get(turn.start)?.fixtureId
      : source.TX[turn.sid]?.[at]?.k === 'u'
        ? turn.sid + ':' + at
        : null;
    if (!fixtureTurn) {
      report.errors.push(
        `Historical frame key missing for ${turn.sid}:${turn.id} at transcript entry ${at}`,
      );
    }
    let referenceTurn = null;
    try {
      referenceTurn = originalTurnId(turn, referenceModel, handoffIds);
    } catch (error) {
      report.errors.push(error.message);
    }
    const stableTurn = fixtureTurn ?? turn.id;
    list.push({
      name: 'trace-' + turn.sid + '-' + String(stableTurn).replace(/[^\w-]/g, '_'),
      route: { v: 'trace', sid: turn.sid, turn: turn.id },
      referenceRoute: referenceTurn ? { v: 'trace', sid: turn.sid, turn: referenceTurn.id } : null,
      referenceTurnBinding: {
        candidateTurn: turn.id,
        candidateSession: turn.sid,
        candidateAt: turn.at ?? null,
        candidateStart: handoffEvidence(
          D.H.find((handoff) => handoff.id === turn.start),
          turn.start,
        ),
        candidateInput: inputEvidence(turn),
        referenceTurn: referenceTurn?.id ?? null,
        referenceAt: referenceTurn?.at ?? null,
        referenceStart: handoffEvidence(
          referenceModel?.handoffs.find((handoff) => handoff.id === referenceTurn?.start),
          referenceTurn?.start,
        ),
        referenceInput: inputEvidence(referenceTurn),
        historicalFrameKey: stableTurn,
      },
    });
  }
  return list;
}

async function navigateReference(page, route, referenceData) {
  const title = titleOf(route, referenceData);
  if (!title)
    throw new Error(`pinned reference route has no qualified title: ${JSON.stringify(route)}`);
  if (page.captureReadinessEnabled) page.captureRouteKey = JSON.stringify(route);
  await page.evaluate((value) => {
    history.pushState(value, '');
    dispatchEvent(new PopStateEvent('popstate', { state: value }));
  }, route);
  try {
    await page.waitForFunction(
      ({ value, title }) => {
        const panel = document.querySelector('#page');
        return (
          history.state?.v === value.v &&
          document.querySelector('#topbar .t')?.textContent === title &&
          panel?.childElementCount > 0 &&
          !panel.hasAttribute('aria-busy') &&
          !panel.querySelector('[aria-busy="true"]') &&
          (value.v !== 'session' ||
            (panel.querySelector('.ph h1')?.textContent === title &&
              !!panel.querySelector('section[aria-label="Transcript"], .transcript'))) &&
          (value.v !== 'trace' ||
            (!!panel.querySelector('.trace-summary') &&
              [...panel.querySelectorAll('.hop.k-root[data-turn]')].some(
                (node) => node.dataset.turn === value.turn,
              )))
        );
      },
      { value: route, title },
    );
  } catch (error) {
    const state = await page.evaluate(() => {
      const panel = document.querySelector('#page');
      return {
        historyState: history.state,
        pathname: location.pathname,
        title: document.querySelector('#topbar .t')?.textContent ?? null,
        busy:
          !panel || panel.hasAttribute('aria-busy') || !!panel.querySelector('[aria-busy="true"]'),
        traceSummary: !!panel?.querySelector('.trace-summary'),
        pageText: panel?.innerText.slice(0, 600) ?? null,
      };
    });
    state.apiDiagnostics = page.captureApiEvents?.slice(-20) ?? [];
    throw new Error(
      `pinned reference route ${JSON.stringify(route)} did not settle; ` +
        `state=${JSON.stringify(state)}; pageErrors=${JSON.stringify(page.errors ?? [])}; ` +
        `wait=${error.message}`,
    );
  }
  if (route.v === 'analytics')
    await page.waitForSelector('.analytics-metrics[data-analytics-ready]');
  await page.evaluate(() => document.fonts.ready);
  await waitForCaptureReady(page, `pinned reference route ${route.v}`);
}

async function navigate(page, route, referenceData) {
  if (selfParity) await goto(page, route, referenceData);
  else await navigateReference(page, route, referenceData);
}

async function prepareFrame(page, phone) {
  await page.evaluate(async () => {
    await document.fonts.ready;
    const state = history.state,
      main = document.querySelector('#main');
    if (state?.v === 'session' && !state.turn) {
      if (matchMedia('(max-width: 760px)').matches)
        window.scrollTo(0, document.documentElement.scrollHeight);
      else if (main) main.scrollTop = main.scrollHeight;
    } else {
      window.scrollTo(0, 0);
      if (main) main.scrollTop = 0;
    }
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  });
  await waitForCaptureReady(page, `frame scroll ${page.url()}`);
  return page.screenshot({ fullPage: phone, animations: 'disabled', caret: 'hide' });
}

async function captureFrame(candidate, reference, key, phone) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    prefix = fileKey(key),
    artifactPaths = {
      candidateImage: `pixels/${prefix}-candidate.png`,
      candidateEvidence: `pixels/${prefix}-candidate.json`,
      referenceImage: `pixels/${prefix}-reference.png`,
      referenceEvidence: `pixels/${prefix}-reference.json`,
      diffImage: `pixels/${prefix}-diff.png`,
      comparisonEvidence: `pixels/${prefix}-comparison.json`,
    };
  for (const [name, page] of [
    ['candidate', candidate],
    ['reference', reference],
  ]) {
    try {
      const bytes = await prepareFrame(page, phone);
      save(key, name, bytes);
      evidenceBySide[name] = await snapshot(page);
      artifactDigests[name] = {
        image: sha256(bytes),
        evidence: saveJson(key, name, evidenceBySide[name]),
      };
      images.push(PNG.sync.read(bytes));
    } catch (error) {
      report.errors.push(`${key} ${name} screenshot: ${error.message}`);
    }
  }
  if (images.length !== 2) return;
  recordDomDiffSummary(
    report.domDiffSummary,
    compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
  );
  const result = compare(...images, THRESHOLD);
  const diffBytes = PNG.sync.write(result.diff);
  save(key, 'diff', diffBytes);
  artifactDigests.diff = {
    image: sha256(diffBytes),
    evidence: saveJson(key, 'comparison', {
      key,
      pixels: result.pixels,
      ratio: result.ratio,
      size: result.size,
      threshold: THRESHOLD,
      maxRatio: MAX_RATIO,
    }),
  };
  report.results.push({
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    size: result.size,
    pass: !result.size && result.ratio <= MAX_RATIO,
    artifactDigests,
    artifactPaths,
  });
}

async function regionEvidence(page, region) {
  return page.evaluate(({ selector, open }) => {
    const roots = [...document.querySelectorAll(selector)];
    const nodes = roots.flatMap((root) => [root, ...root.querySelectorAll('*')]).slice(0, 180);
    return {
      historyState: history.state,
      pathname: location.pathname,
      viewport: [window.innerWidth, window.innerHeight],
      selector,
      count: roots.length,
      openSelector: open ?? null,
      openCount: open ? document.querySelectorAll(open).length : null,
      nodes: nodes.map((node) => {
        const box = node.getBoundingClientRect(),
          css = getComputedStyle(node);
        return {
          tag: node.tagName,
          id: node.id,
          class: node.getAttribute('class'),
          label: node.getAttribute('aria-label'),
          text: [...node.childNodes]
            .filter((child) => child.nodeType === Node.TEXT_NODE)
            .map((child) => child.textContent.slice(0, 300))
            .join(''),
          box: [box.x, box.y, box.width, box.height],
          style: Object.fromEntries(
            ['display', 'font', 'color', 'backgroundColor', 'gap', 'padding', 'overflow'].map(
              (key) => [key, css[key]],
            ),
          ),
        };
      }),
    };
  }, region);
}

async function regionShot(page, region, key, side) {
  if (region.top || region.name === 'transcript') await resetTopScroll(page);
  if (region.open) {
    const opener = page.locator(region.open).first();
    if (!(await opener.count())) throw new Error(`open control missing: ${region.open}`);
    await opener.click();
    await waitForCaptureReady(page, `${key} opened region`);
  }
  const evidence = await regionEvidence(page, region);
  const evidenceDigest = saveJson(key, side + '-geometry', evidence);
  if (!evidence.count) throw new Error(`region selector missing: ${region.selector}`);
  const target = page.locator(region.selector).first();
  await target.waitFor({ state: 'visible', timeout: 3000 });
  const clip = region.name === 'transcript' ? await target.boundingBox() : null;
  if (region.name === 'transcript' && !clip)
    throw new Error('transcript region has no capture box');
  const bytes = clip
    ? await page.screenshot({ clip, animations: 'disabled', caret: 'hide' })
    : await target.screenshot({ animations: 'disabled', caret: 'hide' });
  if (region.open) await closeRegionPanel(page, region, key);
  save(key, side, bytes);
  return {
    image: PNG.sync.read(bytes),
    digests: { image: sha256(bytes), evidence: evidenceDigest },
    evidence,
  };
}

async function closeRegionPanel(page, region, key) {
  // The original app's document Escape listener removes the menu node before
  // its native close owner can release viewerEl/history. Use its real Close
  // button for capture cleanup; keyboard dismissal has its functional suite.
  await page
    .locator(region.selector)
    .first()
    .getByRole('button', { name: 'Close', exact: true })
    .click();
  await page.waitForFunction(
    () => !document.querySelector('dialog[open]') && !history.state?.sheet,
  );
  await waitForCaptureReady(page, `${key} closed region`);
}

async function resetTopScroll(page) {
  await page.evaluate(() => {
    window.scrollTo(0, 0);
    const main = document.querySelector('#main');
    if (main) main.scrollTop = 0;
  });
  await page.evaluate(
    () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))),
  );
  await waitForCaptureReady(page, 'region top scroll');
}

async function sharedTranscriptViewport(candidate, reference, region) {
  const pages = [candidate, reference],
    original = pages.map((page) => page.viewportSize());
  if (
    !original[0] ||
    !original[1] ||
    original[0].width !== original[1].width ||
    original[0].height !== original[1].height
  )
    throw new Error('candidate and reference transcript viewports must match before capture');
  await Promise.all(pages.map(resetTopScroll));
  const boxes = await Promise.all(
    pages.map((page) => page.locator(region.selector).first().boundingBox()),
  );
  if (boxes.some((box) => !box))
    throw new Error('transcript is missing while sizing shared viewport');
  const needed = await Promise.all(
    pages.map(async (page, index) => {
      const dock = page.locator('.session-dock').first(),
        dockHeight = (await dock.count()) ? ((await dock.boundingBox())?.height ?? 0) : 0;
      return boxes[index].y + boxes[index].height + dockHeight + 80;
    }),
  );
  const height = Math.ceil(Math.max(original[0].height, ...needed)),
    shared = original.map((viewport) => ({ width: viewport.width, height }));
  try {
    await Promise.all(pages.map((page, index) => page.setViewportSize(shared[index])));
    await Promise.all(
      pages.map((page) =>
        page.evaluate(async () => {
          await document.fonts.ready;
          await new Promise((resolve) =>
            requestAnimationFrame(() => requestAnimationFrame(resolve)),
          );
        }),
      ),
    );
    await Promise.all(pages.map((page) => waitForCaptureReady(page, 'transcript viewport resize')));
    const stabilized = await Promise.all(
      pages.map((page) => page.locator(region.selector).first().boundingBox()),
    );
    if (
      stabilized.some(
        (box, index) =>
          !box ||
          Math.abs(box.width - boxes[index].width) > 0.5 ||
          Math.abs(box.height - boxes[index].height) > 0.5,
      )
    )
      throw new Error('transcript geometry changed while matching capture viewports');
    const viewports = pages.map((page) => page.viewportSize());
    if (
      viewports.some(
        (viewport) =>
          !viewport || viewport.width !== original[0].width || viewport.height !== height,
      )
    )
      throw new Error('candidate and reference transcript viewports failed to match');
    const requiredAfter = await Promise.all(
      pages.map(async (page, index) => {
        const dock = page.locator('.session-dock').first(),
          dockHeight = (await dock.count()) ? ((await dock.boundingBox())?.height ?? 0) : 0;
        return stabilized[index].y + stabilized[index].height + dockHeight + 80;
      }),
    );
    if (requiredAfter.some((required) => required > height))
      throw new Error('shared transcript viewport does not fit both capture regions');
    const actualViewports = await Promise.all(
      pages.map((page) => page.evaluate(() => [window.innerWidth, window.innerHeight])),
    );
    if (
      actualViewports.some(
        ([width, viewportHeight]) => width !== original[0].width || viewportHeight !== height,
      )
    )
      throw new Error('browser transcript viewports failed to match');
    return original;
  } catch (error) {
    await Promise.all(pages.map((page, index) => page.setViewportSize(original[index])));
    throw error;
  }
}

async function captureRegion(candidate, reference, region, key) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    prefix = fileKey(key),
    artifactPaths = {
      candidateImage: `pixels/${prefix}-candidate.png`,
      candidateEvidence: `pixels/${prefix}-candidate-geometry.json`,
      referenceImage: `pixels/${prefix}-reference.png`,
      referenceEvidence: `pixels/${prefix}-reference-geometry.json`,
      diffImage: `pixels/${prefix}-diff.png`,
      comparisonEvidence: `pixels/${prefix}-comparison.json`,
    };
  const pages = [candidate, reference],
    originalViewports =
      region.name === 'transcript'
        ? await sharedTranscriptViewport(candidate, reference, region)
        : null;
  try {
    for (const [side, page] of [
      ['candidate', candidate],
      ['reference', reference],
    ]) {
      try {
        const captured = await regionShot(page, region, key, side);
        images.push(captured.image);
        artifactDigests[side] = captured.digests;
        evidenceBySide[side] = captured.evidence;
      } catch (error) {
        report.errors.push(`${key} ${side}: ${error.message}`);
      }
    }
    if (images.length !== 2) return;
    recordDomDiffSummary(
      report.domDiffSummary,
      compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
    );
    const result = compare(...images, THRESHOLD);
    const diffBytes = PNG.sync.write(result.diff);
    save(key, 'diff', diffBytes);
    artifactDigests.diff = {
      image: sha256(diffBytes),
      evidence: saveJson(key, 'comparison', {
        key,
        pixels: result.pixels,
        ratio: result.ratio,
        size: result.size,
        threshold: THRESHOLD,
        maxRatio: MAX_RATIO,
      }),
    };
    report.regions.push({
      key,
      pixels: result.pixels,
      ratio: result.ratio,
      size: result.size,
      pass: !result.size && result.ratio <= MAX_RATIO,
      artifactDigests,
      artifactPaths,
    });
  } finally {
    if (originalViewports)
      await Promise.all(pages.map((page, index) => page.setViewportSize(originalViewports[index])));
  }
}

async function openDrawer(page) {
  await page.click('#lead-btn');
  await waitForCaptureReady(page, 'drawer opened');
}

try {
  const referenceBase = selfParity ? parityBase : selectedBase,
    referenceToken = selfParity ? parityToken : selectedToken;
  if (!ENV.base || !ENV.token) throw new Error('Candidate server is required');
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? ''))
    throw new Error('Candidate commit SHA is required for exact visual evidence');
  const candidateBinary = process.env.SEMON_VISUAL_CANDIDATE_BIN ?? process.env.SEMON_BIN;
  if (!candidateBinary || !fs.existsSync(candidateBinary))
    throw new Error('Candidate binary is required for exact visual evidence');
  report.candidateFingerprint = sha256(fs.readFileSync(candidateBinary));
  if (ENV.base === referenceBase)
    throw new Error('Candidate and reference servers must be independent');
  browser = await launch();
  const D = await data(),
    original = sample('overhaul.json'),
    pinnedModel = selfParity ? null : await pinnedReferenceModel(referenceBase, referenceToken);
  report.referenceModelDigest = pinnedModel?.digest ?? null;
  const handoffIds = referenceHandoffs(D, original, pinnedModel?.model),
    routes = screens(D, original, handoffIds, pinnedModel?.model),
    routeByName = new Map();
  for (const route of routes) {
    if (routeByName.has(route.name))
      report.errors.push('Duplicate generated frame route: ' + route.name);
    routeByName.set(route.name, route);
  }
  const selectedNames = new Set(expectedFrames);
  report.routeMappings = routes.map((route) => ({
    frame: route.name,
    candidateRoute: route.route,
    referenceRoute: Object.hasOwn(route, 'referenceRoute') ? route.referenceRoute : route.route,
    ...(route.referenceTurnBinding ? { referenceTurnBinding: route.referenceTurnBinding } : {}),
  }));
  if (expectedFrames.includes('drawer'))
    report.routeMappings.push({
      frame: 'drawer',
      candidateRoute: { v: 'home' },
      referenceRoute: { v: 'home' },
      openControl: '#lead-btn',
    });
  for (const name of expectedFrames)
    if (name !== 'drawer' && !routeByName.has(name))
      report.errors.push('Missing generated frame route: ' + name);
  if (scope === 'main')
    for (const route of routes)
      if (!selectedNames.has(route.name))
        report.errors.push('Unmapped generated frame route: ' + route.name);
  if (scope === 'two-machines')
    for (const route of routes)
      if (
        (route.name === 'machines' || route.name.startsWith('machine-')) &&
        !selectedNames.has(route.name)
      )
        report.errors.push('Unmapped multi-machine frame route: ' + route.name);
  const referenceData = selfParity
    ? await data({ base: parityBase, token: parityToken })
    : {
        SESS: pinnedModel.model.sessions,
        MACHINE: Object.fromEntries(
          (pinnedModel.model.machines ?? [pinnedModel.model.machine]).map((machine) => [
            machine.id,
            machine.name,
          ]),
        ),
      };

  const configured = coverage.frames.map((key) => key.split('|')[0]);
  const schemes = [...new Set(configured)];
  for (const scheme of schemes) {
    const [size, colorScheme] = scheme.split('-'),
      dark = colorScheme === 'dark',
      phone = size === 'phone',
      options = {
        size: phone ? 'phone' : 'desktop',
        viewport: { width: phone ? 390 : 1280, height: phone ? 844 : 860 },
        dark,
      };
    const candidate = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'candidate',
      }),
      reference = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: selfParity ? 'candidate' : 'reference',
        base: referenceBase,
        token: referenceToken,
        ...(selfParity ? {} : { path: '/?compat=1' }),
      });
    const retired = candidate.requests ?? [];
    for (const frame of expectedFrames) {
      if (frame === 'drawer' && !phone) continue;
      const key = `${scheme}|${frame}`;
      if (frame === 'drawer') {
        try {
          await goto(candidate, { v: 'home' }, D);
          await navigate(reference, { v: 'home' }, referenceData);
          await Promise.all([openDrawer(candidate), openDrawer(reference)]);
          await captureFrame(candidate, reference, key, phone);
        } catch (error) {
          report.errors.push(`${key} navigation: ${error.message}`);
        }
      } else {
        const route = routeByName.get(frame);
        if (!route) continue;
        try {
          await goto(candidate, route.route, D);
          const referenceRoute = selfParity
            ? route.route
            : Object.hasOwn(route, 'referenceRoute')
              ? route.referenceRoute
              : route.route;
          if (!referenceRoute) throw new Error('original reference route mapping is unavailable');
          await navigate(reference, referenceRoute, referenceData);
          await captureFrame(candidate, reference, key, phone);
          for (const region of coverage.regions
            .map((item) => item.split('|'))
            .filter(
              ([targetScheme, targetFrame]) => targetScheme === scheme && targetFrame === frame,
            )
            .map(([, , name]) => PIXEL_REGIONS.find((item) => item.name === name))) {
            const regionKey = `${scheme}|${frame}|${region.name}`;
            await captureRegion(candidate, reference, region, regionKey);
          }
        } catch (error) {
          report.errors.push(`${key} navigation: ${error.message}`);
        }
      }
    }
    report.errors.push(
      ...candidate.errors.map((error) => `${scheme} candidate page error: ${error}`),
      ...reference.errors.map((error) => `${scheme} reference page error: ${error}`),
      ...retired
        .filter((url) =>
          /^\/api\/(?:model|transcript|expand|tx|tool|image)(?:\/|$)/.test(new URL(url).pathname),
        )
        .map((url) => `${scheme} retired candidate request: ${new URL(url).pathname}`),
    );
    for (const [side, page] of [
      ['candidate', candidate],
      ['reference', reference],
    ]) {
      report.apiDiagnostics.push(
        ...page.captureApiEvents.map((event) => ({ scheme, side, ...event })),
      );
      if (page.captureApiEventsTruncated)
        report.errors.push(
          `${scheme} ${side} API diagnostics truncated after ${page.captureApiEventsTruncated} events`,
        );
    }
    await candidate.context().close();
    await reference.context().close();
  }
} catch (error) {
  report.errors.push(error.stack ?? error.message);
} finally {
  await browser?.close();
  report.failures = assessPixelComparison(report);
  report.acceptance = assessVisualAcceptances(report, 'pixels', report.failures);
  fs.writeFileSync(path.join(ENV.out, 'pixels.json'), JSON.stringify(report, null, 2) + '\n');
  fs.writeFileSync(path.join(OUT, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(
  JSON.stringify({
    scope,
    frames: report.results.length,
    regions: report.regions.length,
    coverageFailures: report.failures.coverage.length,
    visualFailures: report.failures.visual.length,
    acceptedVisualFailures: report.acceptance.accepted.length,
    unapprovedVisualFailures: report.acceptance.pending.length,
    acceptanceFailures: report.acceptance.errors.length,
  }),
);
if (report.failures.coverage.length || !report.acceptance.pass) process.exitCode = 1;
