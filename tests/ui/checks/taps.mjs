// Tap targets on the phone (#54 point 1): every interactive element on the main phone screens is at least 44×44 CSS px
// at 390 wide, and the check fails listing each one that isn't. It walks Home, Sessions, Machines, Analytics (and its
// slice sheet), every session (collapsed, scrolled up to show the jump button, and with everything expanded), every
// trace, and the overlays a phone reaches from them: the navigation drawer with its tree open, the details menu, the
// filters popover, Find, the session-details sheet with its cost breakdown, the runs sheet, the lineage menu and the
// View script sheet. It runs on the sample fixture and on the extras fixture (code mode, markdown, the payload lane).
//
// What counts as interactive: links, buttons, form controls, summaries, anything with a button, link, menuitem, tab,
// treeitem, option, checkbox or switch role, and anything else focusable by tabindex except scroll regions. A native
// checkbox or radio inside a label is measured through its label, the row a finger actually hits.
//
// Allow-list (each entry a reason; only what a design change, not padding, would be needed to fix):
//   - inline text links inside prose (a[href] whose display is inline): their box is a text line by nature.
//   - chart columns (rect.chart-hit): a column's slot is (chart width - axis) / bins wide, 8-15 px at 390 for 24-30
//     bins, so 44 px wide is not possible without changing the chart's interaction (a scrub or a list beside it).
//     They are 140 px tall. Named in the PR body of #54's tap-target point.
import { ENV, served, goto, data, reporter } from "../lib.mjs";

const MIN = 44;
// A control that a finger can hit, in the page's own terms.
const INTERACTIVE = [
  "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary", "label:has(input, select, textarea)",
  '[role="button"]', '[role="link"]', '[role="menuitem"]', '[role="tab"]', '[role="treeitem"]', '[role="option"]',
  '[role="checkbox"]', '[role="switch"]', '[role="radio"]',
  '[tabindex]:not([tabindex="-1"]):not([role="region"])',
].join(", ");

// Runs in the page: the visible interactive elements, each with its box, a signature and, if allow-listed, why.
function measure({ selector, min }) {
  const cls = (e) => e.tagName.toLowerCase() + [...e.classList].map((c) => "." + c).join("");
  const vw = document.documentElement.clientWidth;
  const out = [];
  for (const e of document.querySelectorAll(selector)) {
    if (e.closest("[hidden], [inert]")) continue;
    if (!e.getClientRects().length) continue;
    const style = getComputedStyle(e);
    if (style.visibility === "hidden" || style.display === "none") continue;
    if ((e.type === "checkbox" || e.type === "radio") && e.closest("label")) continue;
    const rect = e.getBoundingClientRect();
    if (!rect.width || !rect.height) continue;
    // A control may reach past its drawn box with an absolutely positioned ::before (the segmented controls, #117): the
    // finger's target is that reach, so it is measured, not the drawn 24 px.
    const before = getComputedStyle(e, "::before"), reach = (k) => (before.position === "absolute" && before.content !== "none" ? Math.max(0, -parseFloat(before[k]) || 0) : 0);
    const box = { left: rect.left - reach("left"), right: rect.right + reach("right"), width: rect.width + reach("left") + reach("right"), height: rect.height + reach("top") + reach("bottom") };
    // Off screen sideways: the closed drawer, or a carousel: not something a finger can reach yet.
    if (box.right <= 0 || box.left >= vw) continue;
    let allowed = null;
    if (e.matches("a[href]") && style.display === "inline") allowed = "inline text link in prose";
    else if (e.matches("rect.chart-hit")) allowed = "chart column: its slot is narrower than 44 px by construction";
    const text = (e.getAttribute("aria-label") || e.textContent || e.getAttribute("title") || e.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 48);
    out.push({
      sig: (e.parentElement ? cls(e.parentElement) + " > " : "") + cls(e) + (e.matches("[role]") && !e.matches("button, a, input, select") ? "[role=" + e.getAttribute("role") + "]" : ""),
      text, w: Math.round(box.width * 10) / 10, h: Math.round(box.height * 10) / 10, allowed,
      short: box.width < min - 0.05 || box.height < min - 0.05,
    });
  }
  return out;
}

export default async function tapsCheck(browser) {
  const r = reporter("taps");
  const seen = new Map(); // signature -> { w, h, n, screens, text, allowed }
  const problems = [];
  let measured = 0, screens = 0;
  const bySignature = {};

  const tally = async (page, where) => {
    const els = await page.evaluate(measure, { selector: INTERACTIVE, min: MIN });
    screens++; measured += els.length;
    for (const e of els) {
      bySignature[e.sig] = (bySignature[e.sig] ?? 0) + 1;
      if (!e.short || e.allowed) continue;
      const g = seen.get(e.sig) ?? { minW: e.w, minH: e.h, n: 0, screens: new Set(), text: e.text };
      g.minW = Math.min(g.minW, e.w); g.minH = Math.min(g.minH, e.h); g.n++; g.screens.add(where);
      seen.set(e.sig, g);
    }
  };
  // Something a screen must open to be measured: a failure to open it is a failure of the check, not a silent skip.
  const attempt = async (what, fn) => {
    try { await fn(); } catch (e) { problems.push("could not " + what + ": " + String(e.message).split("\n")[0]); }
  };
  const closeOverlays = async (page) => {
    for (let i = 0; i < 3; i++) {
      const open = await page.evaluate(() => !!document.querySelector("dialog[open], .menu:not([hidden]), .filters.pop:not([hidden]), .lineage-menu, body.drawer-open"));
      if (!open) return;
      await page.keyboard.press("Escape"); await page.waitForTimeout(120);
    }
  };
  const expandAll = (page) => page.evaluate(() => {
    for (let k = 0; k < 3; k++) {
      document.querySelectorAll('.cw-toggle[aria-expanded="false"], .tsum[aria-expanded="false"], .step > button[aria-expanded="false"]').forEach((x) => x.click());
    }
    document.querySelectorAll(".hcard .more:not([hidden]), .hop .more:not([hidden]), .child-intro .more:not([hidden]), .child-work .show-all:not([hidden])").forEach((x) => x.click());
    document.querySelectorAll(".think").forEach((x) => x.click());
  });
  const menuAction = async (page, label) => {
    await page.click("#more-btn");
    await page.locator('.menu [role="menuitem"]').filter({ hasText: label }).click();
  };

  for (const [name, extras] of [["sample", false], ["extras", true]]) {
    if (extras && !ENV.extraBase) continue;
    const D = await data({ extras });
    const page = await served(browser, { size: "phone", extras });
    const at = (s) => name + " " + s;

    // The four top-level screens, and Analytics with its slice sheet.
    await tally(page, at("home"));
    for (const v of ["sessions", "machines"]) { await goto(page, { v }, D); await tally(page, at(v)); }
    for (const m of Object.keys(D.MACHINE)) { await goto(page, { v: "machine", id: m }, D); await tally(page, at("machine " + m)); }
    await goto(page, { v: "analytics" }, D); await tally(page, at("analytics"));
    await attempt(name + " analytics range and measure", async () => {
      await page.click('#topbar .analytics-range button:has-text("30 d")'); await page.waitForTimeout(80); await tally(page, at("analytics 30d"));
      await page.click('.analytics-measure button:has-text("API-equivalent cost")'); await page.waitForTimeout(80); await tally(page, at("analytics cost"));
    });
    await attempt(name + " analytics slice sheet", async () => {
      await page.locator(".analytics-panel .chart-hit[role=button]").first().click();
      await page.waitForSelector("dialog.analytics-slice[open]"); await tally(page, at("analytics slice")); await closeOverlays(page);
    });
    // The navigation drawer on Home, with the session tree opened.
    await goto(page, { v: "home" }, D);
    await attempt(name + " navigation drawer", async () => {
      await page.click("#lead-btn"); await page.waitForFunction(() => document.body.classList.contains("drawer-open")); await page.waitForTimeout(350);
      await tally(page, at("drawer"));
      await page.evaluate(() => document.querySelectorAll('[data-tree-toggle][aria-expanded="false"]').forEach((x) => x.click()));
      await page.waitForTimeout(150); await tally(page, at("drawer tree open")); await closeOverlays(page);
    });

    // Every session, and its overlays.
    let detailsDone = 0;
    for (const sid of Object.keys(D.SESS)) {
      const short = D.SESS[sid].name.slice(0, 24);
      await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(80);
      await tally(page, at("session " + short));
      await attempt("scroll " + short + " up for the jump button", async () => {
        await page.evaluate(() => window.scrollBy(0, -Math.min(600, document.documentElement.scrollHeight / 3)));
        await page.waitForTimeout(250); await tally(page, at("session scrolled " + short));
      });
      await expandAll(page); await page.waitForTimeout(100); await tally(page, at("session open " + short));
      await attempt("open the details menu of " + short, async () => {
        await page.click("#more-btn"); await page.waitForSelector(".menu:not([hidden]) [role=menuitem]"); await tally(page, at("details menu " + short));
        await page.locator('.menu [role="menuitem"]').filter({ hasText: "Filter transcript" }).click();
        await page.waitForSelector(".filters.pop:not([hidden])"); await tally(page, at("filters " + short)); await closeOverlays(page);
      });
      await attempt("use Find in " + short, async () => {
        await menuAction(page, "Find in transcript"); await page.waitForSelector("#find"); await tally(page, at("find " + short));
        await page.click('.topbar [aria-label="Close search"]'); await page.waitForTimeout(80);
      });
      // Details are the same sheet for every session: two of them are enough, the first with the cost breakdown open.
      if (detailsDone < 2) await attempt("open the details sheet of " + short, async () => {
        await menuAction(page, "Session details"); await page.waitForSelector("dialog.session-details[open]");
        await tally(page, at("details sheet " + short));
        if (await page.locator("dialog.session-details .cost-row").count()) {
          await page.locator("dialog.session-details .cost-row").click(); await page.waitForTimeout(100); await tally(page, at("details cost " + short));
        }
        await closeOverlays(page); detailsDone++;
      });
      if (await page.locator("#topbar .meta-runs").isVisible()) await attempt("open the runs sheet of " + short, async () => {
        await page.click("#topbar .meta-runs"); await page.waitForSelector("dialog.runs-sheet[open]"); await tally(page, at("runs sheet " + short)); await closeOverlays(page);
      });
      if (await page.locator(".topbar .lineage-parent").count()) await attempt("open the lineage menu of " + short, async () => {
        await page.click(".topbar .lineage-parent"); await page.waitForSelector(".lineage-menu"); await tally(page, at("lineage menu " + short)); await closeOverlays(page);
      });
      if (await page.locator(".step > .out:not([hidden]) .viewscript").count()) await attempt("open View script in " + short, async () => {
        await page.locator(".step > .out:not([hidden]) .viewscript").first().click(); await page.waitForSelector("dialog.viewer[open]");
        await tally(page, at("view script " + short)); await closeOverlays(page);
      });
      // Every trace this session starts, collapsed and with its "Show more" links open.
      const traces = await page.evaluate(() => [...document.querySelectorAll(".turn-end .tracebtn")].map((b) => b.closest(".turn").dataset.turn));
      for (const t of traces) {
        await goto(page, { v: "trace", sid, turn: t }, D); await tally(page, at("trace " + t.slice(0, 16)));
        await expandAll(page); await page.waitForTimeout(80); await tally(page, at("trace open " + t.slice(0, 16)));
      }
    }
    for (const e of page.errors) problems.push(name + " page error: " + e);
    await page.context().close();
  }

  // The account menu, where the embedding server serves one.
  if (ENV.accountBase) {
    const page = await served(browser, { size: "phone", account: true });
    await tally(page, "account home");
    await attempt("open the account menu", async () => {
      // On a phone the account widget sits at the foot of the navigation drawer.
      await page.click("#lead-btn"); await page.waitForFunction(() => document.body.classList.contains("drawer-open")); await page.waitForTimeout(350);
      await tally(page, "account drawer");
      await page.locator(".account-widget-phone .account-trigger").click(); await page.waitForTimeout(150); await tally(page, "account menu"); await closeOverlays(page);
    });
    await page.context().close();
  }

  const shortList = [...seen.entries()].sort((a, b) => a[1].minH - b[1].minH);
  r.results = {
    screens, measured,
    under44: shortList.map(([sig, g]) => ({ sig, minW: g.minW, minH: g.minH, n: g.n, text: g.text, screens: [...g.screens].slice(0, 6) })),
    signatures: bySignature, problems,
  };
  for (const [sig, g] of shortList) {
    r.expect(false, sig + " is " + g.minW + "×" + g.minH + " at its smallest (" + g.n + " on the screens measured, e.g. “" + g.text + "” on " + [...g.screens].slice(0, 3).join("; ") + ")");
  }
  for (const p of problems) r.expect(false, p);
  // The check is not vacuous: it reached the screens it names, and measured the controls those screens are made of.
  r.expect(screens > 60, "measured only " + screens + " screens");
  for (const must of ["tracebtn", "nav-item", "srow", "cost-breakdown-head", "vclose", "jump-bottom", "analytics-range", "analytics-measure", "viewscript", "account-trigger"]) {
    r.expect(Object.keys(bySignature).some((sig) => sig.includes("." + must)), "no ." + must + " was measured: its screen was not reached");
  }
  return r.done();
}
