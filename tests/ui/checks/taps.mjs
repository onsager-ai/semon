// Tap targets on the phone (#54 point 1): every interactive element on the main phone screens is at least 44×44 CSS px
// at 390 wide, and the check fails listing each one that isn't. It walks Home, Sessions, Machines, Analytics (and its
// slice sheet), representative sessions (collapsed, scrolled up to show the jump button, and expanded), their
// trace, and the overlays a phone reaches from them: the navigation drawer with its tree open, the details menu, the
// filters popover, Find, the session-details sheet with its cost breakdown, the runs sheet, the session path in a child's
// details menu and the View script sheet. It runs on the sample fixture and on the extras fixture (code mode, markdown, the payload lane).
//
// A tap target is more than a box, so each control's target (its box and its ::before reach) is hit-tested too: the point 2 px (a pinned sidebar row clips its top pixel)
// inside the midpoint of each of its four edges must land on the control, on something inside it, on the label around it, or on
// another control (rows that stack share their edge). Anything else there (a result line, a card, a clipped ancestor) takes
// the tap away from it, and the check fails. Points under a fixed or sticky bar are not judged: the page scrolls from under it.
// Coarse pointers are measured at phone, landscape-phone and tablet sizes; every layout must retain finger-sized targets.
//
// What counts as interactive: links, buttons, form controls, summaries, anything with a button, link, menuitem, tab,
// treeitem, option, checkbox or switch role, and anything else focusable by tabindex except scroll regions. A form control
// inside a label is measured through its label, the row a finger actually hits.
//
// Allow-list (each entry a reason; only what a design change, not padding, would be needed to fix):
//   - inline text links and tip anchors inside running text (an inline a[href], or the .cr-item runs of a session's status line
//     (#133), whose parent also holds text of its own): their box is a text line by nature. The runs are 17-30 px wide and a
//     thin space apart, so no inline reach makes them 44 px wide; each is a secondary affordance whose data (tool calls, times,
//     the cost and its breakdown) is in the Session details sheet, where every row and the breakdown are measured. A link that
//     stands alone in its element is measured like any control. The allowed controls are counted per signature in taps.json
//     (results.allowed), so a new allowed signature shows.
//   - chart columns only when their panel also offers the equivalent time-slice button list.
import { tapSessions } from "../suite-plan.mjs";
import { ENV, served, goto, data, reporter, closePage } from "../lib.mjs";

const MIN = 44;
// A control that a finger can hit, in the page's own terms.
const INTERACTIVE = [
  "a[href]", "button", "input:not([type=hidden])", "select", "textarea", "summary", "label:has(input, select, textarea)",
  '[role="button"]', '[role="link"]', '[role="menuitem"]', '[role="tab"]', '[role="treeitem"]', '[role="option"]',
  '[role="checkbox"]', '[role="switch"]', '[role="radio"]',
  '[tabindex]:not([tabindex="-1"]):not([role="region"])',
].join(", ");

// Runs in the page: the visible interactive elements, each with its target, a signature and, if allow-listed, why; then, for
// each that is 44 px, the hit-test of its four edge midpoints. The page is scrolled for the hit-test and put back after it.
export function measure({ selector, min }) {
  const cls = (e) => e.tagName.toLowerCase() + [...e.classList].map((c) => "." + c).join("");
  const sigOf = (e) => (e.parentElement ? cls(e.parentElement) + " > " : "") + cls(e) + (e.matches("[role]") && !e.matches("button, a, input, select") ? "[role=" + e.getAttribute("role") + "]" : "");
  const vw = document.documentElement.clientWidth, vh = document.documentElement.clientHeight;
  // A control may reach past its drawn box with an absolutely positioned ::before (the segmented controls, #117; the steps; the
  // order chips): the finger's target is that reach, so it is measured, not the drawn 24 px.
  const target = (e) => {
    const rect = e.getBoundingClientRect(), before = getComputedStyle(e, "::before");
    const reach = (k) => (before.position === "absolute" && before.content !== "none" ? Math.max(0, -parseFloat(before[k]) || 0) : 0);
    return { left: rect.left - reach("left"), right: rect.right + reach("right"), top: rect.top - reach("top"), bottom: rect.bottom + reach("bottom"), width: rect.width + reach("left") + reach("right"), height: rect.height + reach("top") + reach("bottom") };
  };
  const items = [];
  // Native showModal() makes the rest of the document implicitly inert without an inert attribute.
  const modal = [...document.querySelectorAll("dialog:modal")].at(-1);
  const backdrop = document.querySelector(".account-backdrop");
  const account = backdrop?.getClientRects().length ? backdrop.parentElement.querySelector(".account-popover") : null;
  const drawer = document.body.classList.contains("drawer-open") ? document.querySelector(".sidebar") : null;
  const closedDetails = (e) => {
    for (let n = e.parentElement; n; n = n.parentElement) {
      if (!n.matches("details:not([open])")) continue;
      const summary = [...n.children].find((child) => child.matches("summary"));
      if (!summary?.contains(e)) return true;
    }
    return false;
  };
  for (const e of document.querySelectorAll(selector)) {
    if (e.closest("[hidden], [inert]") || closedDetails(e) || (modal && !modal.contains(e)) || (account && !account.contains(e)) || (!modal && !account && drawer && !drawer.contains(e))) continue;
    if (!e.getClientRects().length) continue;
    const style = getComputedStyle(e);
    if (style.visibility === "hidden" || style.display === "none") continue;
    if (e.matches("input, select, textarea") && e.closest("label")) continue;
    const rect = e.getBoundingClientRect();
    if (!rect.width || !rect.height) continue;
    const box = target(e);
    // Off screen sideways: the closed drawer, or a carousel: not something a finger can reach yet.
    if (box.right <= 0 || box.left >= vw) continue;
    let allowed = null;
    if (e.matches("a[href], .cr-item") && style.display === "inline" && e.parentElement && [...e.parentElement.childNodes].some((n) => n.nodeType === 3 && n.textContent.trim())) allowed = "inline text link or tip anchor in running text";
    else if (e.matches("rect.chart-hit") && e.closest(".analytics-panel")?.querySelector(".chart-slices")) allowed = "chart column with equivalent 44 px time-slice buttons";
    const text = (e.getAttribute("aria-label") || e.textContent || e.getAttribute("title") || e.getAttribute("placeholder") || "").trim().replace(/\s+/g, " ").slice(0, 48);
    items.push({
      e, allowed, text, sig: sigOf(e), w: Math.round(box.width * 10) / 10, h: Math.round(box.height * 10) / 10,
      short: box.width < min - 0.05 || box.height < min - 0.05,
    });
  }
  // The hit-test. Scroll positions are noted first and restored after: a screen is measured where it stands.
  const saved = [[document.scrollingElement, document.scrollingElement.scrollTop]];
  for (const x of document.body.querySelectorAll("*")) if (x.scrollHeight > x.clientHeight + 1 || x.scrollTop > 0) saved.push([x, x.scrollTop]);
  // A bar or a sheet the page scrolls from under: forgiven only when it does not hold the control itself (the drawer, a dialog and
  // a menu inside the sticky top bar hold theirs, so a hit inside them is judged like any other).
  // A point on the control's own drawn box that lies outside a scroll container's or a clip's visible box is the part of the control that
  // is scrolled out of it: nobody taps that. A point in its reach that a clip cuts off is not forgiven (the reach is what fails there).
  const clipped = (e, x, y) => { const own = e.getBoundingClientRect(); if (x < own.left || x >= own.right || y < own.top || y >= own.bottom) return false; for (let n = e.parentElement; n && n !== document.documentElement; n = n.parentElement) { const o = getComputedStyle(n); if (o.overflowX === "visible" && o.overflowY === "visible") continue; const r = n.getBoundingClientRect(); if ((n.scrollHeight > n.clientHeight || n.scrollWidth > n.clientWidth) && (x < r.left || x >= r.right || y < r.top || y >= r.bottom)) return true; } return false; };
  const chrome = (n, e) => { for (let own = e; own && own !== document.documentElement; own = own.parentElement) { const position = getComputedStyle(own).position; if (position === "fixed" || position === "sticky") return false; } for (; n && n !== document.documentElement; n = n.parentElement) { const p = getComputedStyle(n).position; if ((p === "fixed" || p === "sticky") && !n.contains(e)) return true; } return false; };
  const misses = [], skipped = { viewportDrawn: 0, scrollDrawn: 0, chrome: 0 };
  // A rounded corner is outside the painted/hittable border at (2,2). Place the four corner probes just inside its
  // elliptical arc instead; the four edge midpoints remain unchanged. Expanded pseudo reach uses its own radii.
  const corners = (e, box) => {
    const own = e.getBoundingClientRect(), style = getComputedStyle(e, box.width > own.width + .05 || box.height > own.height + .05 ? "::before" : null);
    const length = (v, n) => v.endsWith("%") ? parseFloat(v) * n / 100 : parseFloat(v) || 0;
    const radii = ["borderTopLeftRadius", "borderTopRightRadius", "borderBottomLeftRadius", "borderBottomRightRadius"].map(k => {
      const p = style[k].split(/\s+/); return [length(p[0], box.width), length(p[1] ?? p[0], box.height)];
    });
    const [tl,tr,bl,br] = radii;
    const scale = Math.min(1, box.width / (tl[0]+tr[0] || 1), box.width / (bl[0]+br[0] || 1), box.height / (tl[1]+bl[1] || 1), box.height / (tr[1]+br[1] || 1));
    return radii.map(([x,y]) => [Math.max(2, x * scale * (1-Math.SQRT1_2) + .5), Math.max(2, y * scale * (1-Math.SQRT1_2) + .5)]);
  };
  for (const it of items) {
    if (it.allowed || it.short) continue;
    it.e.scrollIntoView({ block: "center", inline: "nearest", behavior: "instant" });
    const b = target(it.e), cx = (b.left + b.right) / 2, cy = (b.top + b.bottom) / 2;
    const [tl,tr,bl,br] = corners(it.e, b);
    for (const [side, x, y] of [["top", cx, b.top + 2], ["bottom", cx, b.bottom - 2], ["left", b.left + 2, cy], ["right", b.right - 2, cy], ["top-left", b.left + tl[0], b.top + tl[1]], ["top-right", b.right - tr[0], b.top + tr[1]], ["bottom-left", b.left + bl[0], b.bottom - bl[1]], ["bottom-right", b.right - br[0], b.bottom - br[1]]]) {
      if (x < 0 || y < 0 || x >= vw || y >= vh) {
        const own = it.e.getBoundingClientRect();
        if (x < own.left || x >= own.right || y < own.top || y >= own.bottom) misses.push({ sig:it.sig, text:it.text, side, by:"viewport clips pseudo reach", byText:"" });
        else skipped.viewportDrawn++;
        continue;
      }
      if (clipped(it.e, x, y)) { skipped.scrollDrawn++; continue; }
      const hit = document.elementFromPoint(x, y);
      if (!hit || hit === it.e || it.e.contains(hit)) continue;
      // Another control (one with no control inside it) takes the tap: rows that stack share their edge. So does the label around this one. A card that holds controls does not: tapping it is not tapping this.
      const ctl = hit.closest(selector);
      if (ctl?.contains(it.e) && ctl.matches("label")) continue;
      // Compact disclosures share target edges with their own neighbouring
      // controls, including rounded corners. Keep this local to a tool group
      // or a sidebar row; prose and unrelated controls must still fail.
      const toolRow = it.e.matches('.step > button, .tsum') && ctl?.matches('.step > button, .tsum');
      const toolGroup = it.e.closest('.tgroup, .steps.lone');
      if (toolRow && toolGroup && ctl.closest('.tgroup, .steps.lone') === toolGroup) continue;
      const sidebarRow = it.e.matches('.srow, .tree-toggle') && ctl?.matches('.srow, .tree-toggle');
      const treeRow = it.e.closest('.tree-row');
      if (sidebarRow && treeRow && treeRow === ctl.closest('.tree-row')) continue;
      if (ctl && !ctl.querySelector(selector) && !side.includes("-") && ctl.parentElement === it.e.parentElement) continue;
      if (chrome(hit, it.e)) { skipped.chrome++; continue; }
      misses.push({ sig: it.sig, text: it.text, side, by: sigOf(hit), byText: (hit.textContent || "").trim().replace(/\s+/g, " ").slice(0, 40) });
    }
  }
  for (const [x, top] of saved) x.scrollTop = top;
  return { items: items.map(({ e, ...rest }) => rest), misses, skipped };
}

export default async function tapsCheck(browser) {
  const r = reporter("taps");
  const seen = new Map(); // signature -> { w, h, n, screens, text, allowed }
  const problems = [];
  let measured = 0, screens = 0;
  const bySignature = {};
  const allowedBy = {}; // allow-listed signature -> { reason, n }
  const covered = new Map(); // signature | side | what is there instead -> { text, byText, n, screens }

  const skippedProbes = { viewportDrawn: 0, scrollDrawn: 0, chrome: 0 };
  const tally = async (page, where) => {
    const { items: els, misses, skipped } = await page.evaluate(measure, { selector: INTERACTIVE, min: MIN });
    await page.waitForTimeout(60); // the scroll listeners that the hit-test's scrolling woke settle before the screen is used again
    screens++; measured += els.length;
    for (const key of Object.keys(skippedProbes)) skippedProbes[key] += skipped[key];
    if (screens % 50 === 0) console.log(`taps: ${screens} screens, ${measured} controls`);
    for (const m of misses) {
      const k = m.sig + " | " + m.side + " | " + m.by;
      const g = covered.get(k) ?? { ...m, n: 0, screens: new Set() };
      g.n++; g.screens.add(where); covered.set(k, g);
    }
    for (const e of els) {
      bySignature[e.sig] = (bySignature[e.sig] ?? 0) + 1;
      if (e.allowed) { const g = (allowedBy[e.sig] ??= { reason: e.allowed, n: 0, samples: [] }); g.n++; if (g.samples.length < 3 && !g.samples.some((sample) => sample.text === e.text && sample.screen === where)) g.samples.push({ text: e.text, screen: where }); }
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
      const open = await page.evaluate(() => !!document.querySelector("dialog[open], .menu:not([hidden]), .filters.pop:not([hidden]), body.drawer-open"));
      if (!open) return;
      await page.keyboard.press("Escape"); await page.waitForTimeout(120);
    }
  };
  const expandAll = (page) => page.evaluate(() => {
    for (let k = 0; k < 3; k++) {
      document.querySelectorAll('.cw-toggle[aria-expanded="false"], .tsum[aria-expanded="false"], .step > button[aria-expanded="false"]').forEach((x) => x.click());
    }
    document.querySelectorAll(".event .ev-more:not([hidden]), .hop .more:not([hidden]), .child-intro .more:not([hidden]), .child-work .show-all:not([hidden])").forEach((x) => x.click());
    document.querySelectorAll(".think").forEach((x) => x.click());
  });
  const menuAction = async (page, label) => {
    await page.click("#more-btn");
    await page.locator('.menu [role="menuitem"]').filter({ hasText: label }).click();
  };

  // A brief long enough to be clipped, its "Show more" shown, and a result line under it ("Returned: …", "Result: …"): the case
  // where a control's reach meets the item that follows it. It is built on the cards and hops a screen already has, by hand, as
  // full.mjs does for a spawn card (the clamp's ResizeObserver does not fire when only the text behind a fixed box changes).
  const clipped = (page) => page.evaluate(() => {
    const paras = () => Array.from({ length: 10 }, (_, i) => Object.assign(document.createElement("p"), { textContent: "Paragraph " + i + " of a long brief. " + "It wraps over several lines on a phone. ".repeat(3) }));
    let cards = 0, hops = 0;
    for (const c of document.querySelectorAll(".event")) {
      const br = c.querySelector(":scope > .ev-text"), more = c.querySelector(":scope > .ev-more"); if (!br || !more) continue;
      br.replaceChildren(...paras()); br.classList.add("clipped"); more.hidden = false; c.querySelector(":scope > .result")?.remove();
      const res = document.createElement("span"); res.className = "result"; const b = document.createElement("b"); b.textContent = "Returned: "; res.append(b, "the parser now keeps the last frame."); more.after(res); cards++;
    }
    for (const body of document.querySelectorAll(".hop .body")) {
      const br = body.querySelector(":scope > .brief"), more = body.querySelector(":scope > .more"); if (!br || !more) continue;
      br.replaceChildren(...paras()); br.classList.add("clipped"); more.hidden = false; body.querySelector(":scope > .result")?.remove();
      const res = document.createElement("div"); res.className = "result"; const l = document.createElement("span"); l.className = "rl"; l.textContent = "Result:"; res.append(l, "the parser now keeps the last frame."); more.after(res); hops++;
    }
    return { cards, hops };
  });
  let syntheticCards = 0, syntheticHops = 0, tokenDetails = 0, sliceLists = 0;

  for (const [device, viewport] of [["phone", { width: 390, height: 844 }], ["landscape", { width: 844, height: 390 }], ["tablet", { width: 768, height: 1024 }]]) {
  for (const [name, extras] of [["sample", false], ["extras", true]]) {
    if (extras && !ENV.extraBase) continue;
    const D = await data({ extras });
    const page = await served(browser, { size: "phone", viewport, extras });
    const at = (s) => device + " " + name + " " + s;

    // The four top-level screens, and Analytics with its slice sheet.
    await tally(page, at("home"));
    for (const v of ["sessions", "machines"]) {
      await goto(page, { v }, D); await tally(page, at(v));
    }
    for (const m of Object.keys(D.MACHINE)) { await goto(page, { v: "machine", id: m }, D); await tally(page, at("machine " + m)); }
    await goto(page, { v: "analytics" }, D); await tally(page, at("analytics"));
    await attempt(name + " analytics range and measure", async () => {
      await page.click('#topbar .analytics-range button:has-text("30 d")'); await page.waitForTimeout(80); await tally(page, at("analytics 30d"));
      await page.click('.analytics-measure button:has-text("API-equivalent cost")'); await page.waitForTimeout(80); await tally(page, at("analytics cost"));
    });
    await attempt(name + " analytics time-slice button lists", async () => {
      const lists = page.locator("details.chart-slices");
      const count = await lists.count();
      if (!count) throw new Error("no equivalent time-slice lists were found");
      for (let i = 0; i < count; i++) {
        const list = lists.nth(i);
        await list.locator(":scope > summary").click();
        if (!await list.evaluate((e) => e.open && !!e.querySelector("button"))) throw new Error("time-slice list did not open with buttons");
        await tally(page, at("analytics time-slice list " + i)); sliceLists++;
        await list.locator(":scope > summary").click();
      }
    });
    await attempt(name + " analytics slice sheet", async () => {
      await page.locator(".analytics-panel .chart-hit[role=button]").first().click();
      await page.waitForSelector("dialog.analytics-slice[open]"); await tally(page, at("analytics slice")); await closeOverlays(page);
    });
    // The navigation drawer on Home, with the session tree opened.
    await goto(page, { v: "home" }, D);
    if (await page.locator("#lead-btn:visible").count()) await attempt(name + " navigation drawer", async () => {
      await page.click("#lead-btn"); await page.waitForFunction(() => document.body.classList.contains("drawer-open")); await page.waitForTimeout(350);
      await tally(page, at("drawer"));
      await page.evaluate(() => document.querySelectorAll('[data-tree-toggle][aria-expanded="false"]').forEach((x) => x.click()));
      await page.waitForTimeout(150); await tally(page, at("drawer tree open")); await closeOverlays(page);
    });

    // Representative content shapes and their overlays; exhaustive mode visits every session.
    let detailsDone = 0;
    for (const sid of tapSessions(D.SESS, name, process.env.SEMON_UI_COVERAGE === "exhaustive")) {
      const short = D.SESS[sid].name.slice(0, 24);
      await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(80);
      await tally(page, at("session " + short));
      await attempt("scroll " + short + " up for the jump button", async () => {
        await page.evaluate(() => window.scrollBy(0, -Math.min(600, document.documentElement.scrollHeight / 3)));
        await page.waitForTimeout(250); await tally(page, at("session scrolled " + short));
      });
      const made = await clipped(page); syntheticCards += made.cards;
      if (made.cards) { await page.waitForTimeout(100); await tally(page, at("session clipped brief and result " + short)); }
      await expandAll(page); await page.waitForTimeout(100); await tally(page, at("session open " + short));
      await attempt("open the details menu of " + short, async () => {
        await page.click("#more-btn"); await page.waitForSelector("dialog.session-menu[open]"); await tally(page, at("details menu " + short));
        await page.click("dialog.session-menu .disclose"); await page.waitForTimeout(100);
        if (!await page.locator('dialog.session-menu .disclose[aria-expanded="true"]').count()
          || !await page.locator("dialog.session-menu .tokens:not([hidden])").count()) throw new Error("token details did not expand");
        tokenDetails += await page.locator("dialog.session-menu .tok-line:visible").count();
        await tally(page, at("tokens by model " + short));
        await closeOverlays(page);
      });
      await attempt("use Find in " + short, async () => {
        await page.click("#find-btn"); await page.waitForSelector("#find"); await tally(page, at("find " + short));
        await page.click('.topbar [aria-label="Close find"]'); await page.waitForTimeout(80);
      });
      if (await page.locator(".step > .out:not([hidden]) .viewscript").count()) await attempt("open View script in " + short, async () => {
        await page.locator(".step > .out:not([hidden]) .viewscript").first().click(); await page.waitForSelector("dialog.panel.full[open]");
        await tally(page, at("view script " + short)); await closeOverlays(page);
      });
      // Every trace this session starts, collapsed and with its "Show more" links open.
      const traces = await page.evaluate(() => [...document.querySelectorAll(".turn-end .link")].map((b) => b.closest(".turn").dataset.turn));
      for (const t of traces) {
        await goto(page, { v: "trace", sid, turn: t }, D); await tally(page, at("trace " + t.slice(0, 16)));
        const madeHops = await clipped(page); syntheticHops += madeHops.hops;
        if (madeHops.hops) { await page.waitForTimeout(100); await tally(page, at("trace clipped brief and result " + t.slice(0, 16))); }
        await expandAll(page); await page.waitForTimeout(80); await tally(page, at("trace open " + t.slice(0, 16)));
      }
    }
    for (const e of page.errors) problems.push(name + " page error: " + e);
    await closePage(page);
  }

  // The account menu, where the embedding server serves one.
  }

  if (ENV.accountBase) for (const [device, viewport] of [["phone", { width: 390, height: 844 }], ["landscape", { width: 844, height: 390 }], ["tablet", { width: 768, height: 1024 }]]) {
    const page = await served(browser, { size: "phone", viewport, account: true });
    await tally(page, device + " account home");
    await attempt(device + " account menu", async () => {
      if (viewport.width <= 760) {
        await page.click("#lead-btn"); await page.waitForFunction(() => document.body.classList.contains("drawer-open")); await page.waitForTimeout(350);
        await tally(page, device + " account drawer");
        await page.locator(".account-widget-phone .account-trigger").click();
      } else await page.locator(".account-widget-desktop .account-trigger").click();
      await page.waitForTimeout(150); await tally(page, device + " account menu"); await closeOverlays(page);
    });
    await closePage(page);
  }

  const shortList = [...seen.entries()].sort((a, b) => a[1].minH - b[1].minH);
  r.results = {
    coverage: process.env.SEMON_UI_COVERAGE === "exhaustive" ? "exhaustive" : "representative",
    screens, measured, skippedProbes, tokenDetails, sliceLists,
    under44: shortList.map(([sig, g]) => ({ sig, minW: g.minW, minH: g.minH, n: g.n, text: g.text, screens: [...g.screens].slice(0, 6) })),
    signatures: bySignature, allowed: allowedBy, problems,
  };
  for (const [sig, g] of shortList) {
    r.expect(false, sig + " is " + g.minW + "×" + g.minH + " at its smallest (" + g.n + " on the screens measured, e.g. “" + g.text + "” on " + [...g.screens].slice(0, 3).join("; ") + ")");
  }
  const coveredList = [...covered.values()].sort((x, y) => y.n - x.n);
  r.results.covered = coveredList.map((g) => ({ sig: g.sig, side: g.side, by: g.by, byText: g.byText, n: g.n, text: g.text, screens: [...g.screens].slice(0, 4) }));
  for (const g of coveredList) {
    r.expect(false, g.sig + " loses the tap at the middle of its " + g.side + " edge to " + g.by + (g.byText ? " (“" + g.byText + "”)" : "") + " (" + g.n + " times, e.g. “" + g.text + "” on " + [...g.screens].slice(0, 3).join("; ") + ")");
  }
  for (const p of problems) r.expect(false, p);
  r.expect(syntheticCards > 0, "no card with a brief could be given a clipped brief and a result line");
  r.expect(syntheticHops > 0, "no hop with a brief could be given a clipped brief and a result line");
  // The check is not vacuous: it reached the screens it names, and measured the controls those screens are made of.
  r.expect(screens > 60, "measured only " + screens + " screens");
  r.expect(tokenDetails > 0, "no expanded informational token details were reached");
  r.expect(sliceLists > 0, "no equivalent chart time-slice button list was opened and measured");
  for (const must of ["nav-item", "srow", "ibtn", "jump", "analytics-range", "analytics-measure", "viewscript", "menu-path-item"]) {
    r.expect(Object.keys(bySignature).some((sig) => sig.includes("." + must)), "no ." + must + " was measured: its screen was not reached");
  }
  for (const shape of ["button.agent-row.agent-more", "div.list > button.more", "div.event.waiting > button.link.ev-more"]) {
    r.expect(Object.keys(bySignature).some(sig => sig.includes(shape)), "representative coverage missed control shape: " + shape);
  }
  return r.done();
}
