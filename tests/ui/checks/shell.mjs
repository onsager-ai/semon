// The gallery is inserted into a document loaded from the viewer's origin so its absolute asset paths, CSP, and fonts
// are exercised exactly as they are for an ordinary page served beside the viewer.
import fs from "node:fs";
import path from "node:path";
import { ENV, reporter } from "../lib.mjs";

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
fs.mkdirSync(output, { recursive: true });

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
  await page.route(/.*/, (route) => (
    route.request().url().startsWith(ENV.base + "/") ? route.continue() : route.abort()
  ));
  // Loading the stylesheet with the token sets the server cookie and gives setContent the right same-origin base URL.
  await page.goto(ENV.base + "/shell.css?t=" + ENV.token, { waitUntil: "load" });
  await page.emulateMedia({ colorScheme: scheme });
  await page.setContent(html, { waitUntil: "load" });
  await page.evaluate(() => document.fonts.ready);
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
  const targets = [...document.querySelectorAll("button, a, input")]
    .filter((element) => element.getClientRects().length && getComputedStyle(element).visibility !== "hidden")
    .map((element) => ({ selector: selector(element), height: Math.round(element.getBoundingClientRect().height * 10) / 10 }));
  const smallTargets = targets.filter((target) => target.height < 44);
  const smallInputs = [...document.querySelectorAll("input")]
    .filter((element) => element.getClientRects().length && Number.parseFloat(getComputedStyle(element).fontSize) < 16)
    .map(selector);
  return {
    scrollWidth: document.documentElement.scrollWidth,
    innerWidth,
    right,
    targets,
    smallTargets,
    smallInputs,
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
          const longNameLength = await page.locator(".rows .row:last-child .nm").evaluate((element) => element.textContent.length);
          results[key].longNameLength = longNameLength;
          r.expect(longNameLength === 90, key + " long row name length=" + longNameLength + " expected 90");
        }
        r.expect(audit.scrollWidth <= audit.innerWidth, key + " document scrollWidth=" + audit.scrollWidth + " innerWidth=" + audit.innerWidth);
        r.expect(audit.right.length === 0, key + " elements past the right edge: " + JSON.stringify(audit.right));
        if (mobile) {
          r.expect(audit.smallTargets.length === 0, key + " targets shorter than 44px: " + JSON.stringify(audit.smallTargets));
          r.expect(audit.smallInputs.length === 0, key + " inputs below 16px: " + JSON.stringify(audit.smallInputs));
        }

        if (mobile && name === "shell-gallery") {
          const line = await page.evaluate(() => {
            window.scrollTo(0, document.documentElement.scrollHeight);
            return new Promise((resolve) => requestAnimationFrame(() => resolve(document.querySelector("#topbar").classList.contains("scrolled"))));
          });
          results[key].scrolled = line;
          r.expect(line, key + " top bar did not gain .scrolled after scrolling");
          await page.evaluate(() => window.scrollTo(0, 0));
          await page.waitForTimeout(30);

          await page.click("#lead-btn");
          const drawerOpen = await page.evaluate(() => document.body.classList.contains("drawer-open"));
          const drawerAudit = await geometry(page);
          results[key].drawerGeometry = drawerAudit;
          r.expect(drawerAudit.scrollWidth <= drawerAudit.innerWidth, key + " drawer document scrollWidth=" + drawerAudit.scrollWidth + " innerWidth=" + drawerAudit.innerWidth);
          r.expect(drawerAudit.right.length === 0, key + " drawer elements past the right edge: " + JSON.stringify(drawerAudit.right));
          r.expect(drawerAudit.smallTargets.length === 0, key + " drawer targets shorter than 44px: " + JSON.stringify(drawerAudit.smallTargets));
          await page.screenshot({ path: path.join(output, key + "-drawer.png"), fullPage: true });
          await page.keyboard.press("Escape");
          const drawerAfterEscape = await page.evaluate(() => ({
            open: document.body.classList.contains("drawer-open"),
            focus: document.activeElement?.id,
            expanded: document.querySelector("#lead-btn")?.getAttribute("aria-expanded"),
          }));
          results[key].drawer = { drawerOpen, ...drawerAfterEscape };
          r.expect(drawerOpen, key + " menu button did not open the drawer");
          r.expect(!drawerAfterEscape.open && drawerAfterEscape.focus === "lead-btn" && drawerAfterEscape.expanded === "false", key + " Escape did not close the drawer and restore focus: " + JSON.stringify(drawerAfterEscape));

          await page.click('#remove-form button[type="submit"]');
          const sheet = await page.evaluate(() => {
            const dialog = document.querySelector("#remove-sheet");
            const box = dialog.getBoundingClientRect();
            return { open: dialog.open, bottom: box.bottom, height: box.height, viewport: innerHeight };
          });
          results[key].sheet = sheet;
          r.expect(sheet.open, key + " confirm sheet did not open");
          r.expect(Math.abs(sheet.bottom - sheet.viewport) <= 1, key + " confirm sheet bottom=" + sheet.bottom + " viewport=" + sheet.viewport);
          const openAudit = await geometry(page);
          r.expect(openAudit.scrollWidth <= openAudit.innerWidth, key + " sheet document scrollWidth=" + openAudit.scrollWidth + " innerWidth=" + openAudit.innerWidth);
          r.expect(openAudit.right.length === 0, key + " sheet elements past the right edge: " + JSON.stringify(openAudit.right));
          r.expect(openAudit.smallTargets.length === 0, key + " open sheet targets shorter than 44px: " + JSON.stringify(openAudit.smallTargets));
          await page.screenshot({ path: path.join(output, key + "-sheet.png"), fullPage: true });
          await page.click('#remove-sheet [data-close]');
          await page.waitForFunction(() => !document.querySelector("#remove-sheet").open);

          await page.click('button[data-open="details-sheet"]');
          const detailsOpen = await page.evaluate(() => document.querySelector("#details-sheet").open);
          r.expect(detailsOpen, key + " data-open did not open its sheet");
          const detailsAudit = await geometry(page);
          r.expect(detailsAudit.smallTargets.length === 0, key + " details sheet targets shorter than 44px: " + JSON.stringify(detailsAudit.smallTargets));
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
