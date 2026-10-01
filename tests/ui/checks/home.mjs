// Ported from the mockup's home.js: Home, the sidebar drawer, the Sessions and Machines pages, group-by, and that
// Home's rows and items actually lead somewhere — phone, default (light) colour scheme, plus a small desktop pass
// for two reference screenshots. The original never set colorScheme, so it only ever ran light; this port keeps
// that (bar.mjs and turns.mjs cover phone dark).
//
// The original ran 'sample' (test7.html) and 'real' (test-real.html) tagged. Ported: 'real' is dropped; 'sample'
// drives the served fixture. Every lookup here is by session id or by a live DOM query (no sample handoff/turn id
// is named), so there is no id mapping to do. Per gaps.json there is exactly one machine ('laptop') in this fixture,
// so machinePages has one entry and its screenshot is always taken (the original's condition — 'buildbox' or a
// single machine — reduces to "always" here).
//
// Assertions:
//  - no page errors.
//  - zero overflow on every screen visited: Home, the drawer, Sessions (each grouping), Machines, every machine page.
//  - the sidebar shows the 8 most recent top-level sessions with children nested beneath their parent, and expand/collapse preferences survive reloads.
//  - every grouping on the Sessions page (project/machine/harness) still shows the same row count as ungrouped —
//    grouping reshuffles into sections, it doesn't drop or duplicate rows.
//  - a live-row tap and a Needs-you item tap on Home both navigate away from Home (liveRowOpens/itemOpens change the
//    page's <h1>), and the item's flash lands on the turn its own data-h resolves to.
//  - a machine page's "up" link goes back to Machines (backTo === "Machines"), and every machine page carries no
//    sideways overflow.
import path from "node:path";
import { ENV, served, data, reporter, overflow, settled, goto, wide } from "../lib.mjs";

// The title is in the bar as soon as a session is clicked; the page is ready once it is no longer aria-busy.
const afterTitle = (page, text) => page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t && !document.querySelector("#page").hasAttribute("aria-busy"), text);

export default async function homeCheck(browser) {
  const D = await data();
  const r = reporter("home");
  const spawned = new Set(D.H.filter((h) => h.kind === "spawn").map((h) => h.to));
  const roots = Object.values(D.SESS).filter((s) => s.lane && !spawned.has(s.id)).sort((a, b) => b.last - a.last);
  const parentOf = (sid) => D.SESS[sid]?.parent ?? D.H.find((h) => (h.kind === "spawn" || h.kind === "relay") && h.to === sid)?.from;
  const shownRoots = new Set(roots.slice(0, 8).map((s) => s.id));
  const treeRoot = (sid) => { const seen = new Set(); while (parentOf(sid) && !seen.has(sid)) { seen.add(sid); sid = parentOf(sid); } return sid; };
  // A parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows are listed;
  // the rest, with everything below them, are behind "All N", so those rows aren't in the sidebar tree. A finished child with a waiting
  // or running session below it ranks as that session does.
  const kidsOf = (sid) => Object.values(D.SESS).filter((s) => s.id !== sid && s.parent === sid);
  const below = (sid, seen = new Set([sid])) => kidsOf(sid).flatMap((k) => (seen.has(k.id) ? [] : (seen.add(k.id), [k, ...below(k.id, seen)])));
  const rankOf = (k) => below(k.id).reduce((rank, d) => (d.state === "wait" ? 0 : d.state === "work" ? Math.min(rank, 1) : rank), k.state === "wait" ? 0 : k.state === "work" ? 1 : 2);
  const folded = new Set();
  const foldAll = (sid) => { if (folded.has(sid)) return; folded.add(sid); kidsOf(sid).forEach((k) => foldAll(k.id)); };
  const foldBelow = (sid, seen = new Set()) => {
    if (seen.has(sid)) return; seen.add(sid);
    const kids = kidsOf(sid).map((k) => ({ k, rank: rankOf(k) })).sort((a, b) => a.rank - b.rank || b.k.last - a.k.last), keep = new Set();
    for (const { k, rank } of kids) if (rank < 2 && keep.size < 8) keep.add(k.id);
    for (const { k } of kids) if (keep.size < 3) keep.add(k.id);
    for (const { k } of kids) if (keep.has(k.id)) foldBelow(k.id, seen); else foldAll(k.id);
  };
  Object.values(D.SESS).filter((s) => !s.parent).forEach((s) => foldBelow(s.id));
  const expectedTreeRows = Object.values(D.SESS).filter((s) => shownRoots.has(treeRoot(s.id)) && !folded.has(s.id)).length;
  const treePair = D.H.find((h) => h.kind === "spawn" && roots.slice(0, 8).some((s) => s.id === h.from) && D.SESS[h.to]);
  const out = {};
  const errs = [];

  {
    const page = await served(browser, { size: "phone", dark: false });
    errs.push(...page.errors);
    const over = () => overflow(page);
    const nav = async (go) => { await page.evaluate(() => window.scrollTo(0, 0)); await page.click("#lead-btn"); await page.waitForTimeout(280); await page.click('.nav-item[data-go="' + go + '"]'); await page.waitForTimeout(150); };

    out.home = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, secs: [...document.querySelectorAll(".page .sec-h")].map((s) => s.textContent), needs: document.querySelectorAll(".page .ib:not(.quiet)").length, items: [...document.querySelectorAll(".page .ib")].map((x) => ({ role: x.getAttribute("role"), tab: x.tabIndex, org: x.querySelector(".org")?.textContent.slice(0, 60) ?? null, trace: !!x.querySelector(".tracebtn"), nestedButton: x.tagName === "BUTTON" })), fors: [...document.querySelectorAll(".page .nrow .for")].map((x) => x.textContent.slice(0, 60)), live: document.querySelectorAll(".page .nrow").length, liveWithAct: document.querySelectorAll(".page .nrow .act").length }));
    out.homeOver = await over();
    await page.screenshot({ path: path.join(ENV.out, "sample-home.png") });

    await page.click("#lead-btn"); await page.waitForTimeout(280);
    out.sidebar = await page.evaluate(() => ({ nav: [...document.querySelectorAll(".nav-item")].map((n) => n.textContent), head: document.querySelector(".side-h")?.textContent, treeRole: document.querySelector("#lanes")?.getAttribute("role"), sidebarChips: document.querySelectorAll(".sidebar .groupby button").length, allLink: document.querySelector("#all-sessions")?.textContent, gheads: document.querySelectorAll("#lanes .ghead").length, rows: document.querySelectorAll("#lanes .treeitem").length, roots: document.querySelectorAll("#lanes > .treeitem").length, children: document.querySelectorAll("#lanes .tree-group .treeitem").length, sorted: [...document.querySelectorAll("#lanes > .treeitem .srow .ag")].slice(0, 5).map((a) => a.textContent) }));
    await page.screenshot({ path: path.join(ENV.out, "sample-drawer.png") });
    out.tree = { expectedRoots: Math.min(8, roots.length), expectedTreeRows, pair: treePair ? { parent: treePair.from, child: treePair.to } : null };
    if (treePair) {
      out.tree.collapse = await page.evaluate(({ parent, child }) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent), toggle = item?.querySelector(":scope > .tree-row .tree-toggle"); if (!item || !toggle) return null; if (item.getAttribute("aria-expanded") !== "true") toggle.click(); toggle.click(); return { expanded: item.getAttribute("aria-expanded"), childUnderParent: [...item.querySelectorAll(":scope > .tree-group .treeitem")].some((x) => x.dataset.id === child), stored: JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[parent]?.open }; }, out.tree.pair);
      await page.reload({ waitUntil: "load" }); await settled(page);
      out.tree.collapseReload = await page.evaluate(({ parent, child }) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent); return { expanded: item?.getAttribute("aria-expanded"), groupDisplay: item ? getComputedStyle(item.querySelector(":scope > .tree-group")).display : null, childUnderParent: [...(item?.querySelectorAll(":scope > .tree-group .treeitem") ?? [])].some((x) => x.dataset.id === child), stored: JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[parent]?.open }; }, out.tree.pair);
      out.tree.expand = await page.evaluate(({ parent, child }) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent), toggle = item?.querySelector(":scope > .tree-row .tree-toggle"); if (!item || !toggle) return null; if (item.getAttribute("aria-expanded") !== "true") toggle.click(); return { expanded: item.getAttribute("aria-expanded"), childUnderParent: [...item.querySelectorAll(":scope > .tree-group .treeitem")].some((x) => x.dataset.id === child), stored: JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[parent]?.open }; }, out.tree.pair);
      await page.reload({ waitUntil: "load" }); await settled(page);
      out.tree.expandReload = await page.evaluate(({ parent, child }) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent); return { expanded: item?.getAttribute("aria-expanded"), childUnderParent: [...(item?.querySelectorAll(":scope > .tree-group .treeitem") ?? [])].some((x) => x.dataset.id === child), stored: JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[parent]?.open }; }, out.tree.pair);
      out.tree.keyboard = await page.evaluate(({ parent }) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent); item?.focus(); item?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowLeft", bubbles: true, cancelable: true })); const left = item?.getAttribute("aria-expanded"); item?.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true, cancelable: true })); return { left, right: item?.getAttribute("aria-expanded") }; }, out.tree.pair);
      await page.click("#lead-btn"); await page.waitForTimeout(280);
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem .srow")].find((x) => x.dataset.id === id)?.click(), treePair.from);
      await afterTitle(page, D.SESS[treePair.from].name);
      await page.waitForFunction(() => { const s = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"); return s && s.scrollHeight - s.scrollTop - s.clientHeight <= 1; });
      out.tree.sidebarOpensAtEnd = await page.evaluate(() => { const s = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"); return s.scrollHeight - s.scrollTop - s.clientHeight <= 1; });
    }
    await nav("sessions");
    out.sessionsPage = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, rows: document.querySelectorAll(".page .nrow").length, groupby: [...document.querySelectorAll(".page .groupby button")].map((x) => x.textContent + (x.getAttribute("aria-pressed") === "true" ? "*" : "")) }));
    for (const g of ["project", "machine", "harness"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(60); out["group_" + g] = await page.evaluate(() => [...document.querySelectorAll(".page .sess .sec-h")].map((h) => h.textContent).join(" | ")); }
    out.sessionsGroupRows = {};
    for (const g of ["project", "machine", "harness", "recent"]) { await page.click('.page .groupby button[data-g="' + g + '"]'); await page.waitForTimeout(60); out.sessionsGroupRows[g] = await page.evaluate(() => document.querySelectorAll(".page .nrow").length); }
    out.sessionsOver = await over();
    await nav("machines");
    out.machines = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, sub: document.querySelector(".ph .sub")?.textContent, rows: [...document.querySelectorAll(".page .nrow")].map((r) => r.innerText.replace(/\n/g, " / ")) }));
    out.machinesOver = await over();
    await page.screenshot({ path: path.join(ENV.out, "sample-machines.png") });
    const ms = await page.evaluate(() => [...document.querySelectorAll(".page .nrow")].map((r) => r.dataset.m));
    r.expect(ms.length > 0, "no machine rows found on the Machines page");
    out.machinePages = [];
    for (const m of ms) {
      await nav("machines"); await page.click('.nrow[data-m="' + m + '"]'); await page.waitForTimeout(120);
      const info = await page.evaluate(() => ({ h1: document.querySelector(".page h1")?.textContent, up: document.querySelector(".uplink")?.textContent, crumb: document.querySelector(".topbar .crumb")?.textContent, cur: document.querySelector('.nav-item[aria-current="page"]')?.textContent, secs: [...document.querySelectorAll(".page .sec-h")].map((s) => s.textContent), sub: document.querySelector(".topbar .l2")?.textContent, moves: [...document.querySelectorAll(".page .ib")].map((x) => x.querySelector(".org")?.textContent.slice(0, 60) ?? null) }));
      info.over = await over(); out.machinePages.push(info);
      await page.screenshot({ path: path.join(ENV.out, "sample-machine-" + m + ".png"), fullPage: true });
      await page.goBack(); await page.waitForTimeout(150); info.backTo = await page.evaluate(() => document.querySelector(".page h1")?.textContent);
    }
    // Home rows lead somewhere.
    await nav("home"); const first = await page.$(".page .nrow"); r.expect(!!first, "no live row (.page .nrow) found on Home"); if (first) { await first.click(); await page.waitForFunction(() => history.state?.v === "session" && !!document.querySelector("#page section[aria-label='Transcript']")); out.liveRowOpens = await page.evaluate(() => { const s = matchMedia("(max-width: 760px)").matches ? document.scrollingElement : document.querySelector("#main"); return { title: document.querySelector(".page h1")?.textContent, gap: s.scrollHeight - s.scrollTop - s.clientHeight }; }); }
    await nav("home"); const q = await page.$(".page .ib .q"); r.expect(!!q, "no Needs-you item (.page .ib .q) found on Home"); if (q) { await q.click(); await page.waitForFunction(() => history.state?.v === "session" && !!history.state?.turn && !!document.querySelector('.turn[data-turn="' + CSS.escape(history.state.turn) + '"]')); out.itemOpens = await page.evaluate(() => { const t = document.querySelector(".turn.flash"), bar = document.querySelector("#topbar").getBoundingClientRect(); return { h1: document.querySelector(".page h1")?.textContent, turn: history.state?.turn, flash: t?.dataset.turn, belowBar: !!t && t.getBoundingClientRect().top >= bar.bottom - 1 }; }); }
    errs.push(...page.errors);
    await page.context().close();
  }

  // Desktop, light: two reference screenshots.
  {
    const page = await served(browser, { size: "desktop", dark: false });
    // Wide is set from a session's menu (it only ever widens a transcript); the rail from the sidebar. Both survive a reload.
    await goto(page, { v: "session", id: Object.keys(D.SESS)[0] }, D);
    await wide(page, true); await page.click("#rail-toggle");
    const wideSid = Object.keys(D.SESS)[0];
    await page.reload({ waitUntil: "load" }); await settled(page); await goto(page, { v: "session", id: wideSid }, D);
    out.layout = await page.evaluate(() => ({ wide: document.querySelector("#page").classList.contains("wide-mode"), rail: document.querySelector(".app").classList.contains("rail"), wideStored: localStorage.getItem("semon.wide"), railStored: localStorage.getItem("semon.rail") }));
    await wide(page, false); await page.click("#rail-toggle"); await page.reload({ waitUntil: "load" }); await settled(page); await goto(page, { v: "session", id: wideSid }, D);
    out.layout.offReload = await page.evaluate(() => ({ wide: document.querySelector("#page").classList.contains("wide-mode"), rail: document.querySelector(".app").classList.contains("rail"), wideStored: localStorage.getItem("semon.wide"), railStored: localStorage.getItem("semon.rail") }));
    await page.screenshot({ path: path.join(ENV.out, "desk-home.png") });
    const longSid = Object.keys(D.TX).sort((a, b) => (D.TX[b]?.length ?? 0) - (D.TX[a]?.length ?? 0))[0];
    await page.setViewportSize({ width: 1280, height: 420 });
    await page.click('.nav-item[data-go="sessions"]'); await afterTitle(page, "Sessions");
    out.historyScroll = { listBefore: await page.evaluate(() => { const s = document.querySelector("#main"); s.scrollTop = Math.min(120, s.scrollHeight - s.clientHeight); return s.scrollTop; }) };
    await page.waitForTimeout(100);
    await page.evaluate((id) => [...document.querySelectorAll(".page .nrow")].find((x) => x.dataset.id === id)?.click(), longSid);
    await afterTitle(page, D.SESS[longSid].name);
    out.historyScroll.openEnd = await page.evaluate(() => { const s = document.querySelector("#main"); return s.scrollHeight - s.scrollTop - s.clientHeight; });
    const historyTravel = await page.evaluate(() => { const s = document.querySelector("#main"); return s.scrollHeight - s.clientHeight; });
    await page.mouse.move(640, 200); await page.mouse.wheel(0, -Math.round(historyTravel * 0.55));
    await page.waitForFunction(() => { const s = document.querySelector("#main"); return s.scrollTop < s.scrollHeight - s.clientHeight - 2; });
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    out.historyScroll.readerAt = await page.evaluate(() => { const s = document.querySelector("#main"); return { top: s.scrollTop, max: s.scrollHeight - s.clientHeight }; });
    await page.goBack(); await afterTitle(page, "Sessions"); await page.waitForTimeout(80);
    out.historyScroll.backTop = await page.evaluate(() => document.querySelector("#main").scrollTop);
    await page.goForward(); await afterTitle(page, D.SESS[longSid].name); await page.waitForTimeout(80);
    out.historyScroll.forwardTop = await page.evaluate(() => document.querySelector("#main").scrollTop);
    await page.setViewportSize({ width: 1280, height: 860 });
    await page.click('.nav-item[data-go="machines"]'); await afterTitle(page, "Machines"); await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, "desk-machines.png") });
    errs.push(...page.errors);
    await page.context().close();
  }

  r.results = out;
  r.expect(errs.length === 0, "page errors: " + errs.join(" | "));
  r.expect(out.homeOver === 0, "home overflow=" + out.homeOver);
  r.expect(out.sessionsOver === 0, "sessions overflow=" + out.sessionsOver);
  r.expect(out.machinesOver === 0, "machines overflow=" + out.machinesOver);
  for (const mp of out.machinePages) r.expect(mp.over === 0, "machine page overflow=" + mp.over + " (" + mp.h1 + ")");
  r.expect(out.sidebar.roots === out.tree.expectedRoots, "sidebar top-level rows=" + out.sidebar.roots + " expected up to 8, got " + out.tree.expectedRoots);
  r.expect(out.sidebar.rows === out.tree.expectedTreeRows && out.sidebar.children === out.tree.expectedTreeRows - out.tree.expectedRoots, "sidebar tree does not render the eight most recent top-level sessions with all children nested: " + JSON.stringify({ sidebar: out.sidebar, expectedTreeRows: out.tree.expectedTreeRows, expectedRoots: out.tree.expectedRoots }));
  r.expect(out.sidebar.treeRole === "tree", "sidebar session rows are missing their tree role");
  r.expect(out.sidebar.children > 0 && !!out.tree.pair, "sidebar tree has no child session under a parent");
  if (out.tree.pair) {
    r.expect(out.tree.collapse?.expanded === "false" && out.tree.collapse.childUnderParent && out.tree.collapse.stored === false, "tree collapse was not saved: " + JSON.stringify(out.tree.collapse));
    r.expect(out.tree.collapseReload?.expanded === "false" && out.tree.collapseReload.groupDisplay === "none" && out.tree.collapseReload.childUnderParent && out.tree.collapseReload.stored === false, "tree collapse did not survive reload: " + JSON.stringify(out.tree.collapseReload));
    r.expect(out.tree.expand?.expanded === "true" && out.tree.expand.childUnderParent && out.tree.expand.stored === true, "tree expand did not show its child and save: " + JSON.stringify(out.tree.expand));
    r.expect(out.tree.expandReload?.expanded === "true" && out.tree.expandReload.childUnderParent && out.tree.expandReload.stored === true, "tree expansion did not survive reload: " + JSON.stringify(out.tree.expandReload));
    r.expect(out.tree.keyboard?.left === "false" && out.tree.keyboard.right === "true", "tree arrow keys did not collapse and expand: " + JSON.stringify(out.tree.keyboard));
    r.expect(out.tree.sidebarOpensAtEnd === true, "opening a sidebar session did not land at the end");
  }
  const baseRows = out.sessionsPage.rows;
  for (const g of ["project", "machine", "harness", "recent"]) r.expect(out.sessionsGroupRows[g] === baseRows, "grouping by " + g + " changed the row count: " + out.sessionsGroupRows[g] + " vs " + baseRows);
  r.expect(!!out.liveRowOpens && out.liveRowOpens.title !== "Home" && out.liveRowOpens.gap <= 1, "a Home live row did not open at the end: " + JSON.stringify(out.liveRowOpens));
  r.expect(!!out.itemOpens && out.itemOpens.h1 !== "Home" && !!out.itemOpens.turn && out.itemOpens.flash === out.itemOpens.turn && out.itemOpens.belowBar, "the first Needs-you item did not reveal its own turn: " + JSON.stringify(out.itemOpens));
  r.expect(out.layout?.wide === true && out.layout.rail === true && out.layout.wideStored === "1" && out.layout.railStored === "1", "wide or rail mode did not survive reload: " + JSON.stringify(out.layout));
  r.expect(out.layout?.offReload?.wide === false && out.layout.offReload.rail === false && out.layout.offReload.wideStored === "0" && out.layout.offReload.railStored === "0", "wide or rail mode off state did not survive reload: " + JSON.stringify(out.layout?.offReload));
  r.expect(out.historyScroll?.openEnd <= 1, "opening a Sessions row did not land at the end: " + JSON.stringify(out.historyScroll));
  r.expect(out.historyScroll?.readerAt?.max > 10 && Math.abs(out.historyScroll.backTop - out.historyScroll.listBefore) <= 2 && Math.abs(out.historyScroll.forwardTop - out.historyScroll.readerAt.top) <= 2, "Back or Forward did not restore the reader position: " + JSON.stringify(out.historyScroll));
  for (const mp of out.machinePages) r.expect(mp.backTo === "Machines", "machine page back did not return to Machines: " + mp.backTo);

  return r.done();
}
