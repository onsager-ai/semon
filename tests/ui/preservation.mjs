// Required candidates compare 68 states; historical-before stays at the original 62.
// The reference binary is the exact gitlink of the captured compat=1 UI.
// Pause removals are never painted back into either UI or silently exempted.
import fs from 'node:fs';
import path from 'node:path';
import { PNG } from 'pngjs';
import { compare } from './pixel-diff.mjs';
import {
  ENV,
  launch,
  served,
  goto,
  data,
  waitForCaptureReady,
  homePostTagDiagnostics,
} from './lib.mjs';
import { compareDomEvidence, recordDomDiffSummary } from './dom-evidence.mjs';
import {
  assessPreservation,
  assessVisualAcceptances,
  BEFORE,
  FILTER_STATE,
  REFERENCE,
  THRESHOLD,
  MAX_RATIO,
  WIDTHS,
  SCHEMES,
  sha256,
} from './preservation-contract.mjs';

const args = process.argv.slice(2);
const historical = args.length === 1 && args[0] === '--historical-before';
if (
  (args.length && !historical) ||
  (historical && process.env.SEMON_PRESERVATION_CANDIDATE !== BEFORE)
)
  throw new Error('Only the pinned before candidate may use --historical-before');

const out = path.join(ENV.out, 'preservation');
const MAX_CAPTURE_SCROLL_EVIDENCE = 128;
fs.mkdirSync(out, { recursive: true });
const report = {
  candidate: process.env.SEMON_PRESERVATION_CANDIDATE ?? process.env.GITHUB_SHA,
  candidateFingerprint: null,
  reference: REFERENCE.source,
  referenceHub: REFERENCE.hub,
  mode: historical ? 'historical-before' : 'required',
  threshold: THRESHOLD,
  maxRatio: MAX_RATIO,
  results: [],
  errors: [],
  apiDiagnostics: [],
  captureScrollEvidence: [],
  captureScrollEvidenceTruncated: 0,
  homePostTagDiagnostics: [],
  domDiffSummary: { casesCompared: 0, casesWithChanges: 0, bySelector: {} },
};
let browser;
const homePostPages = [];
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
          [...document.querySelectorAll(selector)].slice(0, 100).map((node) => {
            const box = node.getBoundingClientRect(),
              css = getComputedStyle(node);
            return {
              text: node.textContent.slice(0, 12000),
              box: [box.x, box.y, box.width, box.height],
              font: css.font,
              padding: css.padding,
              gap: css.gap,
              icons: [...node.querySelectorAll('img')].map((image) => image.getAttribute('src')),
              paths: [...node.querySelectorAll('path')].map((p) => p.getAttribute('d')),
            };
          }),
        ]),
      ),
    };
  });
}
async function snapshotChromeDiagnostics(page) {
  return page.evaluate(() => {
    const pseudo = (node, which) => {
      const style = getComputedStyle(node, which);
      if (style.display === 'none' || style.content === 'none' || style.content === 'normal')
        return null;
      return {
        content: style.content.slice(0, 120),
        display: style.display,
        color: style.color,
        background: style.backgroundColor,
        backgroundImage: style.backgroundImage,
        border: style.borderColor,
        opacity: style.opacity,
        filter: style.filter,
        transform: style.transform,
        width: style.width,
        height: style.height,
      };
    };
    const describe = (node) => {
      const box = node.getBoundingClientRect(),
        css = getComputedStyle(node);
      return {
        tag: node.tagName.toLowerCase(),
        id: node.id,
        className: node.getAttribute('class') ?? '',
        role: node.getAttribute('role'),
        text: (node.textContent ?? '').trim().replace(/\s+/g, ' ').slice(0, 160),
        ariaLabel: node.getAttribute('aria-label'),
        ariaExpanded: node.getAttribute('aria-expanded'),
        title: node.getAttribute('title'),
        box: [box.x, box.y, box.width, box.height],
        style: {
          display: css.display,
          visibility: css.visibility,
          position: css.position,
          color: css.color,
          background: css.backgroundColor,
          backgroundImage: css.backgroundImage,
          border: css.borderColor,
          outline: [css.outlineStyle, css.outlineWidth, css.outlineColor],
          opacity: css.opacity,
          font: css.font,
          fontWeight: css.fontWeight,
          fill: css.fill,
          stroke: css.stroke,
          strokeWidth: css.strokeWidth,
          filter: css.filter,
          transform: css.transform,
          textShadow: css.textShadow,
          boxShadow: css.boxShadow,
        },
        state: {
          hidden: !!node.hidden,
          disabled: node.matches(':disabled'),
          hovered: node.matches(':hover'),
          active: node.matches(':active'),
          focused: node.matches(':focus'),
          focusVisible: node.matches(':focus-visible'),
          focusWithin: node.matches(':focus-within'),
        },
        pseudo: { before: pseudo(node, '::before'), after: pseudo(node, '::after') },
      };
    };
    const active = document.activeElement;
    const transcript = document.querySelector('.transcript');
    const transcriptBox = transcript?.getBoundingClientRect();
    const dockBox = document.querySelector('.session-dock')?.getBoundingClientRect();
    const transcriptStyle = transcript ? getComputedStyle(transcript) : null;
    return {
      activeElement: active ? describe(active) : null,
      viewport: {
        width: innerWidth,
        height: innerHeight,
        devicePixelRatio,
        scrollX,
        scrollY,
        mainScrollTop: document.querySelector('#main')?.scrollTop ?? 0,
      },
      transcriptClip:
        transcriptBox && transcriptStyle
          ? {
              box: [transcriptBox.x, transcriptBox.y, transcriptBox.width, transcriptBox.height],
              bottom: transcriptBox.bottom,
              dockTop: dockBox?.top ?? null,
              clipPath: transcriptStyle.clipPath,
              readingClip: transcriptStyle.getPropertyValue('--reading-clip').trim(),
            }
          : null,
      roots: Object.fromEntries(
        ['#topbar', '.session-dock'].map((selector) => {
          const root = document.querySelector(selector),
            nodes = root ? [root, ...root.querySelectorAll('*')].slice(0, 100) : [];
          return [selector, nodes.map(describe)];
        }),
      ),
    };
  });
}
async function readinessSnapshot(page) {
  try {
    return await page.evaluate(() => {
      const rect = (node) => {
          if (!node) return null;
          const box = node.getBoundingClientRect();
          return [box.x, box.y, box.width, box.height].map(
            (value) => Math.round(value * 100) / 100,
          );
        },
        flags = (node) =>
          node
            ? {
                hidden: !!node.hidden,
                ariaHidden: node.getAttribute('aria-hidden'),
                disabled: node.matches(':disabled'),
                box: rect(node),
              }
            : null,
        state = history.state,
        route = {};
      if (state && typeof state === 'object')
        for (const key of ['v', 'id', 'sid', 'turn', 'scrollTop']) {
          const value = state[key];
          if (typeof value === 'string' && value.length <= 128) route[key] = value;
          else if (typeof value === 'number' && Number.isFinite(value)) route[key] = value;
        }
      const page = document.querySelector('#page'),
        main = document.querySelector('#main'),
        transcript = document.querySelector('.transcript');
      return {
        pathname: location.pathname,
        route,
        capturedAtUnixMs: Math.round(performance.timeOrigin + performance.now()),
        fonts: document.fonts?.status ?? 'unavailable',
        scroll: {
          x: scrollX,
          y: scrollY,
          documentHeight: document.scrollingElement?.scrollHeight ?? null,
          mainTop: main?.scrollTop ?? null,
          mainClientHeight: main?.clientHeight ?? null,
          mainScrollHeight: main?.scrollHeight ?? null,
        },
        page: page
          ? {
              box: rect(page),
              ariaBusy: page.hasAttribute('aria-busy'),
              busyDescendants: page.querySelectorAll('[aria-busy="true"], [data-loading="true"]')
                .length,
              entryCount: page.querySelectorAll('[data-entry-key]').length,
            }
          : null,
        transcript: transcript
          ? {
              box: rect(transcript),
              scrollTop: transcript.scrollTop,
              clientHeight: transcript.clientHeight,
              scrollHeight: transcript.scrollHeight,
            }
          : null,
        dock: flags(document.querySelector('.session-dock')),
        jumpSlot: flags(document.querySelector('.session-jump-slot')),
        jumpBottom: flags(document.querySelector('#jump-bottom')),
      };
    });
  } catch (error) {
    return { unavailable: error?.name ?? 'snapshot failed' };
  }
}
function safeReadinessError(error) {
  return String(error?.message ?? error)
    .replace(/https?:\/\/[^\s"'<>]+/g, (value) => {
      try {
        const url = new URL(value);
        return url.origin + url.pathname;
      } catch {
        return '[URL redacted]';
      }
    })
    .replace(/\?[^\s"'<>]*/g, '?[redacted]')
    .replace(/([?&](?:t|token|auth|secret|api[_-]?key)=)[^&\s"'<>]*/gi, '$1[redacted]')
    .slice(0, 800);
}
async function waitForRouteCaptureReady(page, side, key) {
  try {
    await page.waitForFunction(() => {
      const panel = document.querySelector('#page');
      if (
        !history.state?.v ||
        !document.querySelector('#topbar .t, #topbar #find') ||
        !panel ||
        panel.hasAttribute('aria-busy') ||
        panel.querySelector('[aria-busy="true"], [data-loading="true"]')
      )
        return false;
      return (
        history.state.v !== 'session' ||
        (!!panel.querySelector("section[aria-label='Transcript']") &&
          !!panel.querySelector('[data-entry-key]'))
      );
    });
    await page.evaluate(() => document.fonts.ready);
    await waitForCaptureReady(page, `${key} ${side} before scroll reset`);
  } catch (error) {
    const state = await readinessSnapshot(page);
    throw new Error(
      `${key} ${side} before scroll reset readiness failed: ${safeReadinessError(error)}; ` +
        `snapshot=${JSON.stringify(state)}`,
    );
  }
}
function recordApiDiagnostics(pages, key) {
  for (const [name, page] of pages) {
    const from = page.captureApiEventsReported ?? 0;
    report.apiDiagnostics.push(
      ...page.captureApiEvents.slice(from).map((event) => ({ state: key, side: name, ...event })),
    );
    page.captureApiEventsReported = page.captureApiEvents.length;
  }
}

function recordCaptureScrollEvidence(evidence) {
  if (report.captureScrollEvidence.length < MAX_CAPTURE_SCROLL_EVIDENCE)
    report.captureScrollEvidence.push(evidence);
  else report.captureScrollEvidenceTruncated++;
}

async function captureScrollGeometry(page) {
  return page.evaluate(() => {
    const round = (value) => Math.round(value * 100) / 100;
    const box = (node) => {
      if (!node) return null;
      const rect = node.getBoundingClientRect();
      return [rect.x, rect.y, rect.width, rect.height].map(round);
    };
    const flag = (selector) => {
      const node = document.querySelector(selector);
      if (!node) return { present: false };
      return {
        present: true,
        hidden: !!node.hidden,
        ariaHidden: node.getAttribute('aria-hidden'),
        disabled: node.matches(':disabled'),
        box: box(node),
      };
    };
    const state = history.state,
      route = {};
    if (state && typeof state === 'object')
      for (const key of ['v', 'id', 'sid', 'turn']) {
        const value = state[key];
        if (typeof value === 'string' && value.length <= 128) route[key] = value;
      }
    const root = document.scrollingElement,
      main = document.querySelector('#main');
    const mainClientHeight = main?.clientHeight ?? null;
    const mainScrollHeight = main?.scrollHeight ?? null;
    const windowClientHeight = root?.clientHeight ?? null;
    const windowScrollHeight = root?.scrollHeight ?? null;
    return {
      pathname: location.pathname,
      route,
      viewport: { width: innerWidth, height: innerHeight, devicePixelRatio },
      scroll: {
        windowX: round(scrollX),
        windowY: round(scrollY),
        mainTop: main ? round(main.scrollTop) : null,
      },
      main: {
        present: !!main,
        box: box(main),
        clientHeight: mainClientHeight,
        scrollHeight: mainScrollHeight,
        max: main ? Math.max(0, mainScrollHeight - mainClientHeight) : null,
      },
      window: {
        clientHeight: windowClientHeight,
        scrollHeight: windowScrollHeight,
        max: root ? Math.max(0, windowScrollHeight - windowClientHeight) : null,
      },
      fonts: document.fonts?.status ?? 'unavailable',
      jumpSlot: flag('.session-jump-slot'),
      jumpBottom: flag('#jump-bottom'),
    };
  });
}

function fixedCaptureWheelPoint(pages) {
  const viewports = pages.map(([, page]) => page.viewportSize());
  const viewport = viewports[0];
  const width = viewport?.width ?? 0;
  const height = viewport?.height ?? 0;
  return {
    x: Math.max(0, width - 48),
    y: Math.max(0, Math.min(300, height - 48)),
    viewports,
  };
}

async function captureWheelPointValidation(page, point) {
  return page.evaluate(({ x, y }) => {
    const main = document.querySelector('#main');
    const target = document.elementFromPoint(x, y);
    const element = target?.nodeType === Node.ELEMENT_NODE ? target : target?.parentElement;
    const interactive = element?.closest(
      'button, a, input, textarea, select, [role="button"], [contenteditable="true"]',
    );
    let nestedScroller = null;
    for (let node = element; node && node !== main; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (
        /^(auto|scroll|overlay)$/.test(style.overflowY) &&
        node.scrollHeight > node.clientHeight + 1
      ) {
        nestedScroller = {
          tag: node.tagName.toLowerCase(),
          id: node.id || null,
          className: (node.getAttribute('class') ?? '').slice(0, 120),
        };
        break;
      }
    }
    return {
      insideMain: !!main && !!element && main.contains(element),
      inert: !!element && !interactive && !nestedScroller,
      nestedScroller,
      target: element
        ? {
            tag: element.tagName.toLowerCase(),
            id: element.id || null,
            className: (element.getAttribute('class') ?? '').slice(0, 120),
          }
        : null,
    };
  }, point);
}

async function armCaptureWheel(page, direction) {
  await page.evaluate((wheelDirection) => {
    window.__semonCaptureWheelReceipt = null;
    document.addEventListener(
      'wheel',
      (event) => {
        const target =
          event.target?.nodeType === Node.ELEMENT_NODE ? event.target : event.target?.parentElement;
        window.__semonCaptureWheelReceipt = {
          trusted: event.isTrusted,
          direction: wheelDirection,
          deltaY: event.deltaY,
          deltaMode: event.deltaMode,
          target: target
            ? {
                tag: target.tagName.toLowerCase(),
                id: target.id || null,
                className: (target.getAttribute('class') ?? '').slice(0, 120),
              }
            : null,
        };
      },
      { once: true, capture: true, passive: true },
    );
  }, direction);
}

async function waitForCaptureWheel(page, deadline, predicateLabel, predicate, arg = null) {
  const timeout = deadline - Date.now();
  if (timeout <= 0)
    throw new Error(`capture wheel movement deadline exhausted before ${predicateLabel}`);
  await page.waitForFunction(predicate, arg, { timeout });
}

async function normalizeDesktopSessionScroll(pages, key) {
  const evidenceBySide = new Map();
  const errors = [];
  const point = fixedCaptureWheelPoint(pages);
  const viewportMismatch = point.viewports.some(
    (viewport) =>
      !viewport ||
      viewport.width !== point.viewports[0]?.width ||
      viewport.height !== point.viewports[0]?.height,
  );
  const deadline = Date.now() + 8000;
  await Promise.all(
    pages.map(async ([side, page]) => {
      const entry = { state: key, side, protocol: 'trusted +160 then -100000 in desktop #main' };
      const sideErrors = [];
      const fail = (message) => {
        sideErrors.push(message);
        errors.push(message);
      };
      evidenceBySide.set(side, entry);
      entry.point = { x: point.x, y: point.y };
      entry.viewport = page.viewportSize();
      if (viewportMismatch)
        fail(`${key} candidate/reference viewports differ before normalization`);
      entry.pointValidation = await captureWheelPointValidation(page, point).catch((error) => {
        entry.pointValidationError = safeReadinessError(error);
        fail(`${key} ${side} capture wheel point inspection: ${entry.pointValidationError}`);
        return null;
      });
      entry.before = await captureScrollGeometry(page).catch((error) => {
        entry.beforeError = safeReadinessError(error);
        fail(`${key} ${side} pre-wheel geometry: ${entry.beforeError}`);
        return null;
      });
      const beforeMainTop = entry.before?.scroll?.mainTop;
      const expectedMainFlow = (entry.before?.main?.max ?? 0) > 1;
      entry.expectedMainFlow =
        entry.before?.main?.max === null || entry.before?.main?.max === undefined
          ? 'unknown'
          : expectedMainFlow
            ? 'overflow'
            : 'no-overflow';
      entry.noMainFlowAtStart = entry.expectedMainFlow === 'no-overflow';
      if (entry.expectedMainFlow === 'unknown')
        fail(`${key} ${side} cannot determine #main overflow before capture wheels`);
      entry.windowAtTopBefore = Number.isFinite(entry.before?.scroll?.windowY)
        ? entry.before.scroll.windowY <= 1
        : null;

      try {
        await page.mouse.move(point.x, point.y);
      } catch (error) {
        entry.pointerMoveError = safeReadinessError(error);
        fail(`${key} ${side} move to fixed capture wheel point: ${entry.pointerMoveError}`);
      }
      let positiveArmed = false;
      try {
        await armCaptureWheel(page, 'down');
        positiveArmed = true;
      } catch (error) {
        entry.positiveArmError = safeReadinessError(error);
        fail(`${key} ${side} arm positive capture wheel: ${entry.positiveArmError}`);
      }
      try {
        await page.mouse.wheel(0, 160);
      } catch (error) {
        entry.positiveCommandError = safeReadinessError(error);
        fail(`${key} ${side} send positive capture wheel: ${entry.positiveCommandError}`);
      }
      if (positiveArmed && !entry.positiveArmError && !entry.positiveCommandError) {
        try {
          await waitForCaptureWheel(
            page,
            deadline,
            'positive movement',
            ({ beforeMainTop, expectedFlow }) => {
              const receipt = window.__semonCaptureWheelReceipt;
              const mainTop = document.querySelector('#main')?.scrollTop ?? null;
              return (
                receipt?.trusted === true &&
                receipt.direction === 'down' &&
                (window.scrollY > 1 ||
                  !expectedFlow ||
                  (mainTop !== null && mainTop > beforeMainTop + 1))
              );
            },
            {
              beforeMainTop: beforeMainTop ?? 0,
              expectedFlow,
            },
          );
        } catch (error) {
          entry.positiveWaitError = safeReadinessError(error);
          fail(`${key} ${side} positive capture wheel: ${entry.positiveWaitError}`);
        }
      }
      entry.positiveEvent = await page
        .evaluate(() => window.__semonCaptureWheelReceipt)
        .catch(() => null);
      entry.advanced = await captureScrollGeometry(page).catch((error) => {
        entry.advancedError = safeReadinessError(error);
        fail(`${key} ${side} post-positive geometry: ${entry.advancedError}`);
        return null;
      });

      let returnArmed = false;
      try {
        await armCaptureWheel(page, 'up');
        returnArmed = true;
      } catch (error) {
        entry.returnArmError = safeReadinessError(error);
        fail(`${key} ${side} arm return capture wheel: ${entry.returnArmError}`);
      }
      try {
        await page.mouse.wheel(0, -100000);
      } catch (error) {
        entry.returnCommandError = safeReadinessError(error);
        fail(`${key} ${side} send return capture wheel: ${entry.returnCommandError}`);
      }
      if (returnArmed && !entry.returnArmError && !entry.returnCommandError) {
        try {
          await waitForCaptureWheel(
            page,
            deadline,
            'return to top',
            () =>
              window.__semonCaptureWheelReceipt?.trusted === true &&
              window.__semonCaptureWheelReceipt?.direction === 'up' &&
              scrollY <= 1 &&
              (document.querySelector('#main')?.scrollTop ?? 0) <= 1,
          );
        } catch (error) {
          entry.returnWaitError = safeReadinessError(error);
          fail(`${key} ${side} return capture wheel: ${entry.returnWaitError}`);
        }
      }
      entry.returnEvent = await page
        .evaluate(() => window.__semonCaptureWheelReceipt)
        .catch(() => null);
      entry.returned = await captureScrollGeometry(page).catch((error) => {
        entry.returnedError = safeReadinessError(error);
        fail(`${key} ${side} post-return geometry: ${entry.returnedError}`);
        return null;
      });
      try {
        await page.mouse.move(Math.max(0, (entry.viewport?.width ?? 20) - 20), 300);
      } catch (error) {
        entry.pointerRestoreError = safeReadinessError(error);
        fail(`${key} ${side} restore capture pointer: ${entry.pointerRestoreError}`);
      }

      const startMainTop = entry.before?.scroll?.mainTop;
      const advancedMainTop = entry.advanced?.scroll?.mainTop;
      const positiveMoved =
        Number.isFinite(startMainTop) &&
        Number.isFinite(advancedMainTop) &&
        advancedMainTop > startMainTop + 1;
      const positiveChainedToWindow = (entry.advanced?.scroll?.windowY ?? 0) > 1;
      const returnedMainTop = entry.returned?.scroll?.mainTop;
      const returnedWindowY = entry.returned?.scroll?.windowY;
      const returnedToTop =
        Number.isFinite(returnedMainTop) &&
        Number.isFinite(returnedWindowY) &&
        returnedMainTop <= 1 &&
        returnedWindowY <= 1;
      entry.flowObserved = positiveMoved;
      entry.returnedToTop = returnedToTop;
      if (entry.windowAtTopBefore !== true)
        fail(`${key} ${side} window was not confirmed at top before capture wheels`);
      if (!entry.pointValidation?.insideMain || !entry.pointValidation?.inert)
        fail(`${key} ${side} fixed capture wheel point was not inert inside #main`);
      if (entry.positiveEvent?.trusted !== true || entry.positiveEvent.deltaY <= 0)
        fail(`${key} ${side} positive capture wheel was not a trusted downward event`);
      if (entry.returnEvent?.trusted !== true || entry.returnEvent.deltaY >= 0)
        fail(`${key} ${side} return capture wheel was not a trusted upward event`);
      if (positiveChainedToWindow)
        fail(`${key} ${side} positive capture wheel chained to window scroll`);
      if (entry.expectedMainFlow === 'overflow' && !positiveMoved)
        fail(`${key} ${side} expected #main overflow but +160 did not advance it`);
      if (entry.expectedMainFlow === 'no-overflow' && positiveMoved)
        fail(`${key} ${side} unexpected #main movement from a no-overflow start`);
      if (!returnedToTop)
        fail(`${key} ${side} capture wheel pair did not return both scrollers to top`);
      entry.status = sideErrors.length
        ? 'error'
        : entry.expectedMainFlow === 'overflow'
          ? 'main-flow-observed'
          : entry.expectedMainFlow === 'no-overflow'
            ? 'explicit-no-main-flow'
            : 'main-flow-expectation-unknown';
    }),
  );
  for (const entry of evidenceBySide.values()) recordCaptureScrollEvidence(entry);
  return { errors };
}
async function capture(candidate, reference, key, routeHint) {
  const images = [],
    artifactDigests = {},
    evidenceBySide = {},
    pages = [
      ['candidate', candidate],
      ['reference', reference],
    ],
    artifactPaths = {
      candidateImage: `preservation/${key}-candidate.png`,
      candidateEvidence: `preservation/${key}-candidate.json`,
      referenceImage: `preservation/${key}-reference.png`,
      referenceEvidence: `preservation/${key}-reference.json`,
      diffImage: `preservation/${key}-diff.png`,
      comparisonEvidence: `preservation/${key}-comparison.json`,
    };
  const preflight = await Promise.allSettled(
    pages.map(([name, page]) => waitForRouteCaptureReady(page, name, key)),
  );
  const preflightErrors = preflight
    .filter((result) => result.status === 'rejected')
    .map((result) => result.reason?.message ?? String(result.reason));
  if (preflightErrors.length) {
    recordApiDiagnostics(pages, key);
    throw new Error(preflightErrors.join(' | '));
  }
  const desktopSessionCapture =
    routeHint?.v === 'session' &&
    pages.some(([, page]) => (page.viewportSize()?.width ?? 0) >= 761);
  const reset = await Promise.allSettled(
    pages.map(async ([name, page]) => {
      // Both readers open at the latest record. Cancel that initial following
      // transaction with the same real reader input before comparing their heads.
      await page.mouse.move(page.viewportSize().width - 20, 300);
      await page.mouse.wheel(0, -100000);
      await page.keyboard.press('Home');
      await page.evaluate(async () => {
        await document.fonts.ready;
        window.scrollTo(0, 0);
        document.querySelector('#main').scrollTop = 0;
        await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
      });
      await page.waitForFunction(
        () => scrollY <= 1 && document.querySelector('#main').scrollTop <= 1,
      );
      if (!desktopSessionCapture) await waitForCaptureReady(page, `${key} ${name}`);
    }),
  );
  const resetErrors = (
    await Promise.all(
      reset.map(async (result, index) => {
        if (result.status !== 'rejected') return null;
        const [name, page] = pages[index],
          state = await readinessSnapshot(page);
        return (
          `${key} ${name} after scroll reset failed: ${safeReadinessError(result.reason)}; ` +
          `snapshot=${JSON.stringify(state)}`
        );
      }),
    )
  ).filter(Boolean);
  if (desktopSessionCapture) {
    const captureScrollErrors = (await normalizeDesktopSessionScroll(pages, key)).errors;
    const readiness = await Promise.allSettled(
      pages.map(async ([name, page]) => waitForCaptureReady(page, `${key} ${name}`)),
    );
    recordApiDiagnostics(pages, key);
    const readinessErrors = (
      await Promise.all(
        readiness.map(async (result, index) => {
          if (result.status !== 'rejected') return null;
          const [name, page] = pages[index],
            state = await readinessSnapshot(page);
          return (
            `${key} ${name} after scroll normalization readiness failed: ${safeReadinessError(result.reason)}; ` +
            `snapshot=${JSON.stringify(state)}`
          );
        }),
      )
    ).filter(Boolean);
    const errors = [...resetErrors, ...captureScrollErrors, ...readinessErrors];
    if (errors.length) throw new Error(errors.join(' | '));
  } else {
    recordApiDiagnostics(pages, key);
    if (resetErrors.length) throw new Error(resetErrors.join(' | '));
  }
  if (key.endsWith('-' + FILTER_STATE)) {
    for (const [name, page] of pages) {
      const state = await filterCaptureState(page);
      if (!state.open || !state.closeFocused || state.scrollTop !== 0)
        throw new Error(`${key} ${name} lost the shared open/focus/top-scroll state`);
    }
  }
  for (const [name, page] of pages) {
    const bytes = await page.screenshot({ fullPage: false, animations: 'disabled', caret: 'hide' });
    fs.writeFileSync(path.join(out, key + '-' + name + '.png'), bytes);
    evidenceBySide[name] = await snapshot(page);
    const evidence = Buffer.from(JSON.stringify(evidenceBySide[name], null, 2) + '\n');
    fs.writeFileSync(path.join(out, key + '-' + name + '.json'), evidence);
    artifactDigests[name] = { image: sha256(bytes), evidence: sha256(evidence) };
    images.push(PNG.sync.read(bytes));
  }
  const includeChromeDiagnostics =
    /^(390|820)-(light|dark)-(?:session-harbor|harbor-tools-open|harbor-tool-output-open)$/.test(
      key,
    );
  if (includeChromeDiagnostics) {
    for (const [name, page] of pages) {
      evidenceBySide[name].chromeDiagnostics = await snapshotChromeDiagnostics(page);
      const evidence = Buffer.from(JSON.stringify(evidenceBySide[name], null, 2) + '\n');
      fs.writeFileSync(path.join(out, key + '-' + name + '.json'), evidence);
      artifactDigests[name].evidence = sha256(evidence);
    }
  }
  recordDomDiffSummary(
    report.domDiffSummary,
    compareDomEvidence(evidenceBySide.candidate, evidenceBySide.reference, key),
  );
  const result = compare(...images, report.threshold);
  const diffBytes = PNG.sync.write(result.diff);
  fs.writeFileSync(path.join(out, key + '-diff.png'), diffBytes);
  const comparisonEvidence = Buffer.from(
    JSON.stringify(
      {
        key,
        pixels: result.pixels,
        ratio: result.ratio,
        difference: result.difference,
        size: result.size,
        threshold: report.threshold,
        maxRatio: report.maxRatio,
      },
      null,
      2,
    ) + '\n',
  );
  fs.writeFileSync(path.join(out, key + '-comparison.json'), comparisonEvidence);
  artifactDigests.diff = { image: sha256(diffBytes), evidence: sha256(comparisonEvidence) };
  report.results.push({
    key,
    pixels: result.pixels,
    ratio: result.ratio,
    difference: result.difference,
    size: result.size,
    pass: !result.size && result.ratio <= report.maxRatio,
    artifactDigests,
    artifactPaths,
  });
}
async function prepareFilterCapture(page) {
  await page.locator('.facet-btn').click();
  const dialog = page.locator('.filters-sheet'),
    close = dialog.locator('.vclose'),
    body = dialog.locator('.vb');
  await page.waitForFunction(() => document.querySelector('.filters-sheet')?.open === true);
  await close.focus();
  await body.evaluate((node) => {
    node.scrollTop = 0;
  });
  const state = await filterCaptureState(page);
  if (!state.open || !state.closeFocused || state.scrollTop !== 0)
    throw new Error('Filter capture did not reach the shared open/focus/top-scroll state');
}
async function filterCaptureState(page) {
  return page.evaluate(() => {
    const dialog = document.querySelector('.filters-sheet'),
      close = dialog?.querySelector('.vclose'),
      body = dialog?.querySelector('.vb');
    return {
      open: dialog?.open === true,
      closeFocused: document.activeElement === close,
      scrollTop: body?.scrollTop ?? null,
    };
  });
}
async function closeFilterCapture(page) {
  await page.locator('.filters-sheet .vclose').click();
  await page.waitForFunction(
    () =>
      !document.querySelector('.filters-sheet')?.open &&
      !history.state?.sheet &&
      history.state?.v === 'sessions' &&
      document.activeElement === document.querySelector('.facet-btn'),
  );
}
try {
  if (!ENV.base || !process.env.SEMON_PRESERVATION_BASE || !process.env.SEMON_PRESERVATION_TOKEN)
    throw new Error('Both candidate and selected-reference servers are required');
  if (ENV.base === process.env.SEMON_PRESERVATION_BASE.replace(/\/$/, ''))
    throw new Error('Candidate and selected-reference servers must be independent');
  browser = await launch();
  if (!/^[a-f0-9]{40}$/.test(report.candidate ?? ''))
    throw new Error('Candidate commit SHA is required for exact visual evidence');
  const candidateBinary = process.env.SEMON_VISUAL_CANDIDATE_BIN ?? process.env.SEMON_BIN;
  if (!candidateBinary || !fs.existsSync(candidateBinary))
    throw new Error('Candidate binary is required for exact visual evidence');
  report.candidateFingerprint = sha256(fs.readFileSync(candidateBinary));
  const D = await data();
  for (const width of WIDTHS)
    for (const scheme of SCHEMES) {
      const dark = scheme === 'dark';
      const options = {
        size: width === 390 ? 'phone' : 'desktop',
        viewport: { width, height: width === 390 ? 844 : 860 },
        dark,
      };
      const prefix = width + '-' + (dark ? 'dark' : 'light');
      const candidate = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'candidate',
        captureHomePostTag: true,
        onCaptureHomePostTagPage: (page) => homePostPages.push({ state: prefix, page }),
      });
      const retired = candidate.requests ?? [];
      const reference = await served(browser, {
        ...options,
        captureRequests: true,
        captureReadiness: true,
        captureRole: 'reference',
        base: process.env.SEMON_PRESERVATION_BASE,
        token: process.env.SEMON_PRESERVATION_TOKEN,
        path: '/?compat=1',
      });
      for (const route of [
        { v: 'home' },
        { v: 'sessions' },
        { v: 'session', id: 'harbor' },
        { v: 'session', id: 'atlas-ingest' },
        { v: 'session', id: 'deps' },
        { v: 'session', id: 'h-failed' },
        { v: 'analytics' },
      ]) {
        await goto(candidate, route, D);
        reference.captureRouteKey = JSON.stringify(route);
        await reference.evaluate((route) => {
          history.pushState(route, '');
          dispatchEvent(new PopStateEvent('popstate', { state: route }));
        }, route);
        await reference.waitForFunction(
          (route) =>
            history.state.v === route.v &&
            !!document.querySelector('#topbar .t') &&
            (route.v !== 'session' || !!document.querySelector('#page .transcript')),
          route,
        );
        if (route.v === 'analytics')
          await reference.waitForSelector('.analytics-metrics[data-analytics-ready]');
        await capture(
          candidate,
          reference,
          prefix + '-' + route.v + (route.id ? '-' + route.id : ''),
          route,
        );
        if (route.v === 'sessions' && !historical) {
          for (const page of [candidate, reference]) await prepareFilterCapture(page);
          await capture(candidate, reference, prefix + '-' + FILTER_STATE, { v: 'sessions' });
          for (const page of [candidate, reference]) await closeFilterCapture(page);
        }
        if (route.id === 'harbor') {
          for (const page of [candidate, reference])
            await page.locator('.think-disclosure').first().click();
          await capture(candidate, reference, prefix + '-harbor-thinking-open', route);
          for (const page of [candidate, reference]) await page.locator('.tsum').first().click();
          await capture(candidate, reference, prefix + '-harbor-tools-open', route);
          for (const page of [candidate, reference]) {
            await page.locator('.tgroup .steps .step > button').first().click();
            await page.locator('.tgroup .steps .out:not([hidden])').first().waitFor();
          }
          await capture(candidate, reference, prefix + '-harbor-tool-output-open', route);
        }
      }
      if (width === 390) {
        await goto(candidate, { v: 'home' }, D);
        reference.captureRouteKey = JSON.stringify({ v: 'home' });
        await reference.evaluate(() => {
          history.pushState({ v: 'home' }, '');
          dispatchEvent(new PopStateEvent('popstate', { state: { v: 'home' } }));
        });
        for (const page of [candidate, reference]) await page.click('#lead-btn');
        await capture(candidate, reference, prefix + '-drawer', { v: 'home' });
      }
      report.errors.push(
        ...candidate.errors.map((error) => prefix + ' candidate: ' + error),
        ...reference.errors.map((error) => prefix + ' reference: ' + error),
        ...retired
          .filter((url) =>
            /^\/api\/(?:model|transcript|expand|tx|tool|image)(?:\/|$)/.test(new URL(url).pathname),
          )
          .map((url) => prefix + ' retired candidate request: ' + new URL(url).pathname),
      );
      for (const [side, page] of [
        ['candidate', candidate],
        ['reference', reference],
      ]) {
        const from = page.captureApiEventsReported ?? 0;
        report.apiDiagnostics.push(
          ...page.captureApiEvents.slice(from).map((event) => ({ state: prefix, side, ...event })),
        );
        page.captureApiEventsReported = page.captureApiEvents.length;
        if (page.captureApiEventsTruncated)
          report.errors.push(
            `${prefix} ${side} API diagnostics truncated after ${page.captureApiEventsTruncated} events`,
          );
      }
      await candidate.context().close();
      await reference.context().close();
    }
} catch (error) {
  report.errors.push(error.stack ?? error.message);
} finally {
  report.homePostTagDiagnostics = homePostPages.map(({ state, page }) => ({
    state,
    ...homePostTagDiagnostics(page),
  }));
  await browser?.close();
  report.captureScrollEvidenceEntries = report.captureScrollEvidence.length;
  if (report.captureScrollEvidenceTruncated)
    report.errors.push(
      `capture scroll evidence truncated after ${MAX_CAPTURE_SCROLL_EVIDENCE} entries ` +
        `(${report.captureScrollEvidenceTruncated} omitted)`,
    );
  report.failures = assessPreservation(report);
  report.acceptance = assessVisualAcceptances(report, 'preservation', report.failures);
  fs.writeFileSync(path.join(out, 'report.json'), JSON.stringify(report, null, 2) + '\n');
}
console.log(
  JSON.stringify({
    candidate: report.candidate,
    reference: report.reference,
    cases: report.results.length,
    coverageFailures: report.failures.coverage.length,
    visualFailures: report.failures.visual.length,
    acceptedVisualFailures: report.acceptance.accepted.length,
    pendingVisualFailures: report.acceptance.pending.length,
    apiEvents: report.apiDiagnostics.length,
    apiFailures: report.apiDiagnostics.filter(
      (event) =>
        event.event === 'failed' ||
        (event.event === 'response' && !event.ok && event.status !== 304),
    ).length,
    captureScrollEvidenceEntries: report.captureScrollEvidenceEntries,
    captureScrollEvidenceTruncated: report.captureScrollEvidenceTruncated,
    errors: report.errors.length,
  }),
);
// Only the fixed historical before-run is diagnostic. The candidate must satisfy
// pixel, geometry, provenance and exact coverage assertions. Neither native
// qualification nor the control pause grants an implicit pixel waiver.
if (report.failures.coverage.length || (!historical && !report.acceptance.pass))
  process.exitCode = 1;
