// The gallery is inserted into a document loaded from the viewer's origin so its absolute asset paths, CSP, and fonts
// are exercised exactly as they are for an ordinary page served beside the viewer.
import fs from "node:fs";
import path from "node:path";
import { ENV, reporter, served } from "../lib.mjs";

const pages = [
  ["shell-gallery", new URL("../shell-gallery.html", import.meta.url)],
  ["shell-signin", new URL("../shell-signin.html", import.meta.url)],
];
const sizes = [
  ["390", 390, 844, true],
  ["1280", 1280, 800, false],
];
const schemes = ["light", "dark"];
const output = path.join(ENV.out, "shell");
const GALLERY_PATH = "/__shell-gallery.html";
const PHONE_TARGET_SELECTOR = ".btn, .field input, .code .copy, dialog.sheet button";
fs.mkdirSync(output, { recursive: true });

// Diagnostics for the hang on a gallery page's first load (see galleryPage): every request's start and end, so a timeout
// can name the ones still pending, and how many contexts and pages the shared browser holds at that moment.
function trackRequests(page) {
  const started = new Map(), done = [];
  const t0 = Date.now();
  page.on("request", (request) => started.set(request, Date.now() - t0));
  page.on("requestfinished", (request) => { done.push([request.url(), started.get(request), Date.now() - t0, "finished"]); started.delete(request); });
  page.on("requestfailed", (request) => { done.push([request.url(), started.get(request), Date.now() - t0, "failed: " + (request.failure()?.errorText ?? "")]); started.delete(request); });
  return (browser) => {
    const contexts = browser.contexts();
    return JSON.stringify({
      now: Date.now() - t0,
      pending: [...started].map(([request, at]) => ({ url: request.url(), type: request.resourceType(), startedAt: at })),
      done,
      contexts: contexts.length,
      pages: contexts.map((context) => context.pages().map((open) => open.url())),
    });
  };
}

async function galleryPage(browser, html, width, height, mobile, scheme) {
  const context = await browser.newContext({
    viewport: { width, height },
    deviceScaleFactor: mobile ? 2 : 1,
    isMobile: mobile,
    hasTouch: mobile,
    colorScheme: scheme,
    timezoneId: "UTC",
  });
  await context.grantPermissions(["clipboard-read", "clipboard-write"], { origin: ENV.base });
  const page = await context.newPage();
  page.errors = [];
  page.on("pageerror", (error) => page.errors.push(error.message.split("\n")[0]));
  page.setDefaultTimeout(5000);
  const report = trackRequests(page);
  try {
  await page.route(/.*/, (route) => (
    route.request().url().startsWith(ENV.base + "/") ? route.continue() : route.abort()
  ));
  // Loading the stylesheet with the token sets the server cookie and shows which security headers the viewer's origin sends.
  const stylesheet = await page.goto(ENV.base + "/shell.css?t=" + ENV.token, { waitUntil: "load" });
  // The gallery is then navigated to as a page of its own on that origin, answered from the file with those same headers
  // (CSP included), rather than written into the stylesheet's document with page.setContent. Two CI runs (36483037109 and
  // 36528320379) hung in setContent's wait for "load" on the first gallery page, the second after a 30 s timeout, so the
  // wait was not merely slow. The cause was not established (no browser to reproduce it here); a navigation does not
  // go through document.open() on the just-loaded stylesheet document, and serves the same page under the same origin,
  // headers and cookie.
  const headers = Object.fromEntries(Object.entries(stylesheet.headers())
    .filter(([name]) => !["content-type", "content-length", "content-encoding", "etag", "set-cookie", "date", "connection", "transfer-encoding", "keep-alive"].includes(name)));
  await page.route(ENV.base + GALLERY_PATH, (route) => route.fulfill({ status: 200, contentType: "text/html; charset=utf-8", headers, body: html }));
  await page.goto(ENV.base + GALLERY_PATH, { waitUntil: "load" });
  await page.emulateMedia({ colorScheme: scheme });
  await page.evaluate(() => document.fonts.ready);
  } catch (error) {
    const probe = Date.now();
    const answered = await fetch(ENV.base + "/shell.css?t=" + ENV.token, { signal: AbortSignal.timeout(5000) }).then((r) => r.status, (e) => e.name);
    console.log("DIAG probe of /shell.css from node: " + answered + " after " + (Date.now() - probe) + " ms");
    console.log("DIAG galleryPage " + scheme + " " + width + " " + error.message.split("\n")[0] + " " + report(browser));
    throw error;
  }
  return { context, page };
}

const geometry = (page) => page.evaluate(() => {
  const selector = (element) => element.id
    ? "#" + CSS.escape(element.id)
    : element.tagName.toLowerCase() + [...element.classList].map((name) => "." + CSS.escape(name)).join("");
  const width = document.documentElement.clientWidth;
  const right = [];
  for (const element of document.querySelectorAll("*")) {
    const box = element.getBoundingClientRect();
    if (box.width && box.height && box.right > width + 0.5) right.push({ selector: selector(element), right: Math.round(box.right * 10) / 10 });
  }
  const smallInputs = [...document.querySelectorAll("input")]
    .filter((element) => element.getClientRects().length && Number.parseFloat(getComputedStyle(element).fontSize) < 16)
    .map(selector);
  return {
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth,
    right,
    smallInputs,
  };
});

const phoneTargets = (page) => page.evaluate((targetSelector) => {
  const selector = (element) => element.id
    ? "#" + CSS.escape(element.id)
    : element.tagName.toLowerCase() + [...element.classList].map((name) => "." + CSS.escape(name)).join("");
  return [...document.querySelectorAll(targetSelector)]
    .filter((element) => element.getClientRects().length && getComputedStyle(element).visibility !== "hidden")
    .map((element) => ({ selector: selector(element), height: Math.round(element.getBoundingClientRect().height * 10) / 10 }));
}, PHONE_TARGET_SELECTOR);

// Polls a selector's bounding rect (rounded to a tenth of a pixel) until it holds for three consecutive animation
// frames, or a timeout passes. Used so a screenshot or an assertion never lands mid-transition: a CSS transition
// (the drawer's slide, in this codebase) keeps changing the rect every frame until it finishes, so waiting for it
// to stop moving is equivalent to waiting for the transition to end, without depending on a "transitionend" event
// firing for the right property (or at all, if a browser coalesces or skips it).
const stableRect = (page, selector, timeout = 1500) => page.evaluate(({ selector, timeout }) => new Promise((resolve) => {
  const deadline = performance.now() + timeout;
  let lastKey = null, stableFrames = 0;
  const read = () => {
    const element = document.querySelector(selector);
    if (!element) return null;
    const box = element.getBoundingClientRect();
    return { left: box.left, right: box.right, top: box.top, bottom: box.bottom, width: box.width, height: box.height };
  };
  const step = () => {
    const box = read();
    const key = box ? [box.left, box.top, box.width, box.height].map((n) => Math.round(n * 10)).join(",") : "none";
    if (key === lastKey) stableFrames += 1; else { stableFrames = 0; lastKey = key; }
    if (stableFrames >= 3 || performance.now() > deadline) { resolve(box); return; }
    requestAnimationFrame(step);
  };
  step();
}), { selector, timeout });

const chromeDimensions = (page) => page.evaluate(() => {
  const computed = (element) => {
    if (!element) return null;
    const style = getComputedStyle(element);
    return { width: Number.parseFloat(style.width), height: Number.parseFloat(style.height) };
  };
  return {
    lead: computed(document.querySelector("#lead-btn")),
    nav: computed(document.querySelector(".nav-item")),
  };
});

export default async function shellCheck(browser) {
  const r = reporter("shell");
  const results = {};
  const galleryHtml = fs.readFileSync(pages[0][1], "utf8");

  for (const [name, file] of pages) {
    const html = name === "shell-gallery" ? galleryHtml : fs.readFileSync(file, "utf8");
    for (const [widthName, width, height, mobile] of sizes) {
      for (const scheme of schemes) {
        const { context, page } = await galleryPage(browser, html, width, height, mobile, scheme);
        const key = name + "-" + widthName + "-" + scheme;
        results[key] = {};
        await page.screenshot({ path: path.join(output, key + ".png"), fullPage: true });

        const audit = await geometry(page);
        results[key].geometry = audit;
        if (name === "shell-gallery") {
          const longNameLength = await page.locator(".rows .row:last-child .sh-nm").evaluate((element) => element.textContent.length);
          results[key].longNameLength = longNameLength;
          r.expect(longNameLength === 90, key + " long row name length=" + longNameLength + " expected 90");
          // A link styled as a button reads as a button: no underline, and the button's own ink, not the link accent.
          const linkButton = await page.locator('a.btn[href="#learn-more"]').evaluate((element) => {
            const style = getComputedStyle(element), plain = getComputedStyle(document.querySelector(".btn-row button.btn:not(.primary)"));
            return { line: style.textDecorationLine, color: style.color, buttonColor: plain.color };
          });
          results[key].linkButton = linkButton;
          r.expect(linkButton.line === "none", key + " a.btn is underlined: " + JSON.stringify(linkButton));
          r.expect(linkButton.color === linkButton.buttonColor, key + " a.btn colour differs from a button's: " + JSON.stringify(linkButton));
        }
        r.expect(audit.scrollWidth <= audit.innerWidth, key + " document scrollWidth=" + audit.scrollWidth + " innerWidth=" + audit.innerWidth);
        r.expect(audit.right.length === 0, key + " elements past the right edge: " + JSON.stringify(audit.right));
        if (mobile) {
          const targets = await phoneTargets(page);
          const shortTargets = targets.filter((target) => target.height < 44);
          results[key].phoneTargets = targets;
          r.expect(shortTargets.length === 0, key + " shell targets shorter than 44px: " + JSON.stringify(shortTargets));
          r.expect(audit.smallInputs.length === 0, key + " inputs below 16px: " + JSON.stringify(audit.smallInputs));
        }

        if (mobile && name === "shell-gallery") {
          const links = await page.evaluate(() => ["#paragraph-link", "#notice-link"].map((id) => {
            const element = document.querySelector(id);
            if (!element) return { id, missing: true };
            return { id, height: element.getBoundingClientRect().height, fontSize: Number.parseFloat(getComputedStyle(element).fontSize) };
          }));
          results[key].inlineLinks = links;
          r.expect(links.length === 2 && links.every((link) => !link.missing), key + " expected both inline links: " + JSON.stringify(links));
          for (const link of links) {
            if (!link.missing) r.expect(link.height <= link.fontSize * 1.6, key + " " + link.id + " is taller than inline text: " + JSON.stringify(link));
          }

          const viewerPage = await served(browser, { size: "phone", dark: scheme === "dark", path: "/" });
          await viewerPage.waitForSelector("#lead-btn");
          await viewerPage.waitForSelector(".nav-item", { state: "attached" });
          const [galleryChrome, viewerChrome] = await Promise.all([chromeDimensions(page), chromeDimensions(viewerPage)]);
          results[key].chromeDimensions = { gallery: galleryChrome, viewer: viewerChrome };
          for (const part of ["lead", "nav"]) {
            const gallerySize = galleryChrome[part], viewerSize = viewerChrome[part];
            r.expect(!!gallerySize && !!viewerSize && Math.abs(gallerySize.width - viewerSize.width) <= 0.01 && Math.abs(gallerySize.height - viewerSize.height) <= 0.01,
              key + " " + part + " dimensions differ from viewer: " + JSON.stringify({ gallery: gallerySize, viewer: viewerSize }));
          }
          await viewerPage.context().close();

          const line = await page.evaluate(() => {
            window.scrollTo(0, document.documentElement.scrollHeight);
            return new Promise((resolve) => requestAnimationFrame(() => resolve(document.querySelector("#topbar").classList.contains("scrolled"))));
          });
          results[key].scrolled = line;
          r.expect(line, key + " top bar did not gain .scrolled after scrolling");
          await page.evaluate(() => window.scrollTo(0, 0));
          await page.waitForTimeout(30);

          await page.click("#lead-btn");
          const openSidebarRect = await stableRect(page, "#sidebar");
          const drawerOpen = await page.evaluate(() => document.body.classList.contains("drawer-open"));
          const { scrimVisible, innerWidth: viewportWidth } = await page.evaluate(() => {
            const scrim = document.getElementById("scrim");
            const box = scrim?.getBoundingClientRect();
            const style = scrim ? getComputedStyle(scrim) : null;
            const scrimVisible = !!scrim && box.width > 0 && box.height > 0 && Number.parseFloat(style.opacity) > 0 && style.pointerEvents !== "none";
            return { scrimVisible, innerWidth };
          });
          results[key].drawerSidebar = { rect: openSidebarRect, scrimVisible };
          r.expect(!!openSidebarRect && openSidebarRect.left >= -0.5, key + " open drawer sidebar left=" + openSidebarRect?.left + " expected >= 0");
          r.expect(!!openSidebarRect && openSidebarRect.right <= viewportWidth + 0.5, key + " open drawer sidebar right=" + openSidebarRect?.right + " expected <= " + viewportWidth);
          r.expect(!!openSidebarRect && openSidebarRect.width >= 240, key + " open drawer sidebar width=" + openSidebarRect?.width + " expected >= 240");
          r.expect(scrimVisible, key + " scrim is not visible while the drawer is open");
          const drawerAudit = await geometry(page);
          const drawerTargets = await phoneTargets(page);
          const shortDrawerTargets = drawerTargets.filter((target) => target.height < 44);
          results[key].drawerGeometry = drawerAudit;
          r.expect(drawerAudit.scrollWidth <= drawerAudit.innerWidth, key + " drawer document scrollWidth=" + drawerAudit.scrollWidth + " innerWidth=" + drawerAudit.innerWidth);
          r.expect(drawerAudit.right.length === 0, key + " drawer elements past the right edge: " + JSON.stringify(drawerAudit.right));
          r.expect(shortDrawerTargets.length === 0, key + " drawer shell targets shorter than 44px: " + JSON.stringify(shortDrawerTargets));
          // A viewport screenshot, not fullPage: the drawer and scrim are fixed-position, so only the viewport shows
          // them where the user actually sees them.
          await page.screenshot({ path: path.join(output, key + "-drawer.png"), fullPage: false });
          await page.keyboard.press("Escape");
          const closedSidebarRect = await stableRect(page, "#sidebar");
          const drawerAfterEscape = await page.evaluate(() => ({
            open: document.body.classList.contains("drawer-open"),
            focus: document.activeElement?.id,
            expanded: document.querySelector("#lead-btn")?.getAttribute("aria-expanded"),
          }));
          results[key].drawer = { drawerOpen, ...drawerAfterEscape, closedSidebarRect };
          r.expect(drawerOpen, key + " menu button did not open the drawer");
          r.expect(!drawerAfterEscape.open && drawerAfterEscape.focus === "lead-btn" && drawerAfterEscape.expanded === "false", key + " Escape did not close the drawer and restore focus: " + JSON.stringify(drawerAfterEscape));
          r.expect(!!closedSidebarRect && closedSidebarRect.right <= 0.5, key + " sidebar was not fully off screen after closing: " + JSON.stringify(closedSidebarRect));

          await page.click('#remove-form button[type="submit"]');
          await page.waitForFunction(() => document.querySelector("#remove-sheet")?.open === true);
          await stableRect(page, "#remove-sheet");
          const sheet = await page.evaluate(() => {
            const dialog = document.querySelector("#remove-sheet");
            const box = dialog.getBoundingClientRect();
            const main = document.getElementById("main");
            return { open: dialog.open, bottom: box.bottom, height: box.height, viewport: innerHeight, mainLeft: main ? main.getBoundingClientRect().left : null };
          });
          results[key].sheet = sheet;
          r.expect(sheet.open, key + " confirm sheet did not open");
          r.expect(Math.abs(sheet.bottom - sheet.viewport) <= 1, key + " confirm sheet bottom=" + sheet.bottom + " viewport=" + sheet.viewport);
          r.expect(sheet.mainLeft != null && Math.abs(sheet.mainLeft) <= 0.5, key + " main was shifted to left=" + sheet.mainLeft + " while the sheet was open (the page behind it must not move)");
          const openAudit = await geometry(page);
          const sheetTargets = await phoneTargets(page);
          const shortSheetTargets = sheetTargets.filter((target) => target.height < 44);
          r.expect(openAudit.scrollWidth <= openAudit.innerWidth, key + " sheet document scrollWidth=" + openAudit.scrollWidth + " innerWidth=" + openAudit.innerWidth);
          r.expect(openAudit.right.length === 0, key + " sheet elements past the right edge: " + JSON.stringify(openAudit.right));
          r.expect(shortSheetTargets.length === 0, key + " open sheet shell targets shorter than 44px: " + JSON.stringify(shortSheetTargets));
          // A viewport screenshot, not fullPage: dialog.sheet is fixed-position.
          await page.screenshot({ path: path.join(output, key + "-sheet.png"), fullPage: false });
          const clickPoints = await page.evaluate(() => {
            const box = document.querySelector("#remove-sheet").getBoundingClientRect();
            return { paddingX: box.left + 8, paddingY: box.top + box.height / 2, backdropX: innerWidth / 2, backdropY: box.top - 8 };
          });
          await page.mouse.click(clickPoints.paddingX, clickPoints.paddingY);
          const stayedOpen = await page.evaluate(() => document.querySelector("#remove-sheet").open);
          results[key].sheetPaddingClick = { points: clickPoints, stayedOpen };
          r.expect(stayedOpen, key + " click inside sheet padding closed the sheet");
          await page.mouse.click(clickPoints.backdropX, clickPoints.backdropY);
          const backdropClosed = await page.evaluate(() => !document.querySelector("#remove-sheet").open);
          results[key].sheetBackdropClick = { points: clickPoints, closed: backdropClosed };
          r.expect(backdropClosed, key + " click above the sheet did not close it");

          await page.click('button[data-open="details-sheet"]');
          const detailsOpen = await page.evaluate(() => document.querySelector("#details-sheet").open);
          r.expect(detailsOpen, key + " data-open did not open its sheet");
          const detailsTargets = await phoneTargets(page);
          const shortDetailsTargets = detailsTargets.filter((target) => target.height < 44);
          r.expect(shortDetailsTargets.length === 0, key + " details sheet targets shorter than 44px: " + JSON.stringify(shortDetailsTargets));
          await page.click('#details-sheet [data-close]');

          await page.click("[data-copy]");
          await page.waitForFunction(() => document.querySelector("[data-copy]").classList.contains("copied"));
          const copied = await page.evaluate(async () => ({
            value: await navigator.clipboard.readText(),
            label: document.querySelector("[data-copy]").getAttribute("aria-label"),
          }));
          results[key].copy = copied;
          r.expect(copied.value === "example-value-123", key + " copied text was " + JSON.stringify(copied.value));
          r.expect(copied.label === "Copied", key + " copy control did not show Copied feedback");
        }

        results[key].pageErrors = page.errors;
        r.expect(page.errors.length === 0, key + " page errors: " + page.errors.join(" | "));
        await context.close();
      }
    }
  }

  r.results = results;
  return r.done();
}
