// The sidebar tree, on the extras fixture, phone and desktop. It holds two live parents: `Fan-out` (seven subagents, only the
// oldest, Reader 1, still running) and `Swarm` (twelve: Workers 1 to 10 running, 11 and 12 finished and newer).
//   - an open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows
//     are listed, and ends with one "All N" row when anything is hidden, N being the count pill's number. Fan-out lists Reader 1
//     (old, but running) ahead of the newer finished Readers 7 and 6; Swarm lists Workers 10 to 3 and no finished worker.
//   - there is no "Show N more" or "Show fewer" list control, and an old saved `more` is ignored and pruned on the next save.
//   - a parent with no waiting or running session below it, and not holding the open session, is collapsed by default.
//   - while a search is typed the children that match it are listed, capped by the same rule, and "All N" stays when others are hidden.
//   - the keyboard reaches "All N" in tree order; on a phone the row is at least 44 px tall and its text has AA contrast.
//   - on a phone "All N" opens a bottom sheet with a search field and Waiting / Running / Finished sections holding exactly the
//     descendants, newest first; a tap on a row opens that session and closes the sheet; Esc and a tap on the backdrop close it
//     and return focus to "All N"; rows are at least 44 px, there is one scroll surface, no sideways overflow, and AA contrast.
//   - on a wide screen "All N" opens no dialog: it lists the whole parent inline, in the same order; the parent's row sticks to the
//     top of the sidebar while the list scrolls, with "Show fewer", which folds the list, scrolls the parent back into view and
//     focuses it; the open list is not saved (a reload starts short); navigating to a row under the sticky row scrolls it clear.
//   - opening a nested parent's "All N" inside an open parent leaves the ancestors listed and open and sticks only the innermost
//     row; on a wide screen "All N" reveals exactly its number (every descendant, under its own parent, open); a live update keeps
//     the whole list open and focus on "Show fewer"; crossing 760 px, the rail, collapsing the parent or an ancestor, and a list with
//     nothing left to fold all drop the open list; Esc in a phone's sheet leaves the drawer open; the sheet says which parent a
//     grandchild is under.
//   - a collapsed parent's count pill is the bare total with no state dot, fully round and AA in both themes; it is amber (`wait`) exactly when a run inside needs you, and its title and the row's aria-label break the total down (runs, needs you, working, failed). A patched model puts a waiting and a failed run, and a grandchild, under Fan-out.
//   - the toggle sits over the right end of its row's meta line, and only rows with children have one: no row has a left gutter.
//   - the toggle's box is at least 44x44 at 390 px and at least 28x36 at 1280 px, and a collapsed parent's summary does not
//     overlap it.
//   - only the open session's row is current: its parent is not highlighted; a collapsed parent opens for the child's page
//     without saving that, and its saved choice is unchanged after navigating away; a collapse made while the child is open
//     stays collapsed through redraws; the open child is always listed; in the rail the ancestor keeps a ring, no highlight, and
//     aria-current="true".
//   - the sidebar as a whole never scrolls: the header, search and nav keep their boxes while "Recent" scrolls on its own, and
//     "All sessions ›" stays fully inside the screen at 390 and 1280 whether the list overflows or not. A short list leaves
//     the footer right after its last row; a row focused by keyboard scrolls into the list, never under the footer; the rail
//     still toggles and shows no footer.
//   - the nav runs Home, Sessions, Analytics, Machines top to bottom in the expanded sidebar, the rail and the phone drawer.
//   - screenshots of the sidebar (and of the sheet, and of the sticky list) at 390 and 1280, light and dark, go to out/sidebar-*.png.
import path from "node:path";
import { ENV, served, goto, data, reporter, overflow, settled } from "../lib.mjs";

const openDrawer = async (page) => { await page.click("#lead-btn"); await page.waitForTimeout(300); };

// A root's direct children, its "All N" row, its pill, and what is saved for it.
const groupOf = (page, id) => page.evaluate((id) => {
  const item = [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id);
  if (!item) return null;
  const all = item.querySelector(":scope > .tree-group > .tree-all"), summary = item.querySelector(":scope > .tree-row .tree-summary");
  const stored = JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[id] ?? null;
  return {
    ids: [...item.querySelectorAll(":scope > .tree-group > .treeitem")].map((x) => x.dataset.id),
    all: all ? { text: all.textContent, role: all.getAttribute("role"), label: all.getAttribute("aria-label"), tag: all.tagName, tabIndex: all.tabIndex, height: Math.round(all.getBoundingClientRect().height * 10) / 10 } : null,
    oldButtons: document.querySelectorAll("#lanes .tree-more").length, dialogs: document.querySelectorAll("dialog").length,
    pill: summary ? summary.textContent.trim() : null, expanded: item.getAttribute("aria-expanded"), stored,
    stuck: !!item.querySelector(":scope > .tree-row.stuck"), fewer: item.querySelector(":scope > .tree-row .tree-fewer")?.textContent ?? null,
  };
}, id);

// Where the names start within each group of visible rows, and whether any group mixes rows with and without children.
const gutters = (page) => page.evaluate(() => {
  const groups = [], boxes = [document.querySelector("#lanes"), ...document.querySelectorAll("#lanes .tree-group")];
  let mixed = 0;
  for (const box of boxes) {
    const rows = [...box.querySelectorAll(":scope > .treeitem")].map((item) => ({ item, nm: item.querySelector(":scope > .tree-row .nm")?.getBoundingClientRect() })).filter((r) => r.nm && r.nm.width);
    if (rows.length < 2) continue;
    const lefts = rows.map((r) => Math.round(r.nm.left * 10) / 10), ends = [...new Set(rows.map((r) => Math.round(r.item.querySelector(":scope > .tree-row .ag").getBoundingClientRect().right * 10) / 10))], parents = rows.filter((r) => r.item.querySelector(":scope > .tree-row .tree-toggle")).length;
    if (parents && parents < rows.length) mixed++;
    groups.push({ depth: box.dataset.depth ?? "0", lefts: [...new Set(lefts)], ends, rows: rows.length, parents });
  }
  return { groups, mixed, spacers: document.querySelectorAll("#lanes .tree-spacer").length };
});

const toggleBox = (page, id) => page.evaluate((id) => {
  const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id), t = item?.querySelector(":scope > .tree-row .tree-toggle"), row = item?.querySelector(":scope > .tree-row .srow");
  if (!t || !row) return null;
  const a = t.getBoundingClientRect(), b = row.getBoundingClientRect(), line = item.querySelector(":scope > .tree-row").getBoundingClientRect();
  return { w: Math.round(a.width * 10) / 10, h: Math.round(a.height * 10) / 10, right: a.right <= b.right + 0.5 && a.right >= b.right - 12, lower: (a.top + a.bottom) / 2 >= (line.top + line.bottom) / 2 - 1 && a.bottom <= line.bottom + 0.5, color: getComputedStyle(t).color, bg: getComputedStyle(t).backgroundColor };
}, id);

// Every top-level parent with children, collapsed in turn (and put back as it was): its count pill, measured. Contrast is taken
// against the pill's real backdrop.
const collapsedPills = (page) => page.evaluate(() => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  return [...document.querySelectorAll("#lanes > .treeitem")].filter((item) => item.querySelector(":scope > .tree-row .tree-toggle")).map((item) => {
    const toggle = item.querySelector(":scope > .tree-row .tree-toggle"), wasOpen = item.getAttribute("aria-expanded") === "true";
    if (wasOpen) toggle.click();
    const s = item.querySelector(":scope > .tree-row .tree-summary"), row = item.querySelector(":scope > .tree-row .srow");
    let out = { id: item.dataset.id, missing: true };
    if (s) {
      const layers = []; for (let e = s; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor));
      let bg = [255, 255, 255, 1]; for (const layer of layers.reverse()) bg = over(layer, bg);
      const cs = getComputedStyle(s), x = lum(rgba(cs.color)), y = lum(bg);
      out = { id: item.dataset.id, text: s.textContent, dots: s.querySelectorAll(".dot").length, tip: s.dataset.tip, label: row.getAttribute("aria-label"), wait: s.classList.contains("wait"), ratio: Math.round((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05) * 100) / 100, radius: cs.borderRadius, height: s.getBoundingClientRect().height, background: cs.backgroundColor };
    }
    if (wasOpen) toggle.click();
    return out;
  });
});

// What a parent's descendants are doing, from the model: the total, and how many are waiting on you, working or failed.
const tally = (SESS, H, id) => {
  const seen = new Set([id]), queue = [id], all = [];
  const kidsOf = (p) => [...Object.values(SESS).filter((x) => x.parent === p).map((x) => x.id), ...H.filter((h) => h.kind === "spawn" && h.from === p && h.to).map((h) => h.to)];
  while (queue.length) for (const k of kidsOf(queue.shift())) if (SESS[k] && !seen.has(k)) { seen.add(k); queue.push(k); all.push(SESS[k]); }
  const count = (state) => all.filter((x) => x.state === state).length;
  return { total: all.length, wait: count("wait"), work: count("work"), err: count("err") };
};
const breakdown = (t) => [t.total + (t.total === 1 ? " run" : " runs"), t.wait && t.wait + " needs you", t.work && t.work + " working", t.err && t.err + " failed"].filter(Boolean);

// Each pill: the bare total and no dot, fully round, AA against its backdrop; amber (`wait`) exactly when something inside
// needs you, and neutral otherwise (a failure is named in the text, never coloured); tooltip (data-tip) and label break the total down.
function assertPills(r, where, pills, SESS, H, mustShow) {
  r.expect(pills.length > 0 && pills.every((p) => !p.missing), where + ": a collapsed parent shows no count pill: " + JSON.stringify(pills.filter((p) => p.missing)));
  for (const p of pills.filter((x) => !x.missing)) {
    const t = tally(SESS, H, p.id), parts = breakdown(t), name = SESS[p.id]?.name + " ";
    r.expect(p.dots === 0 && p.text === String(t.total), where + " " + name + ": the pill is the bare total with no state dot: " + JSON.stringify(p));
    r.expect(p.tip === parts.join(" · "), where + " " + name + ": the tooltip is " + JSON.stringify(parts.join(" · ")) + ": " + JSON.stringify(p.tip));
    r.expect(p.label?.endsWith(", " + parts.join(", ")), where + " " + name + ": the row's aria-label carries the breakdown: " + JSON.stringify(p.label));
    r.expect(p.wait === t.wait > 0, where + " " + name + ": the wait class is " + p.wait + " with " + t.wait + " waiting");
    r.expect(p.ratio >= 4.5, where + " " + name + ": the pill's text has contrast " + p.ratio + ", under 4.5");
    r.expect(parseFloat(p.radius) >= p.height / 2, where + " " + name + ": the pill is not fully round: radius " + p.radius + " for height " + p.height);
  }
  for (const name of mustShow) r.expect(pills.some((p) => SESS[p.id]?.name === name), where + ": " + name + " has no pill");
}

// The model with a waiting and a failed run under Fan-out and a grandchild under one of its runs, served in place of the real
// one at load (the fixture's sessions derive their state from logs, and only top-level sessions can wait).
const patchModel = (m, kidIds) => {
  m.sessions[kidIds[0]].state = "wait"; m.sessions[kidIds[1]].state = "err";
  const source = m.sessions[kidIds[3]], spawn = m.handoffs.find((h) => h.kind === "spawn" && h.to === kidIds[3]);
  m.sessions["fan-grandchild"] = { ...structuredClone(source), id: "fan-grandchild", name: "Reader helper", parent: kidIds[3], state: "done" };
  if (spawn) m.handoffs.push({ ...structuredClone(spawn), id: "fan-grandchild-spawn", from: kidIds[3], to: "fan-grandchild" });
  return m;
};
const servedPatched = async (browser, opts, kidIds) => {
  const page = await served(browser, opts);
  await page.route((u) => u.pathname === "/api/model" && !u.searchParams.has("since"), async (route) => {
    const res = await route.fetch(), m = await res.json();
    await route.fulfill({ status: res.status(), headers: { "content-type": "application/json" }, body: JSON.stringify(patchModel(m, kidIds)) });
  });
  await page.reload({ waitUntil: "load" }); await settled(page);
  return page;
};
// The sidebar's boxes and the list's scroll state, in one read.
const sideGeometry = (page) => page.evaluate(() => {
  const r4 = (e) => { if (!e) return null; const b = e.getBoundingClientRect(); return [b.left, b.top, b.width, b.height].map((n) => Math.round(n * 10) / 10); };
  const sb = document.querySelector("#sidebar"), list = document.querySelector("#side-list"), all = document.querySelector("#all-sessions");
  const rows = [...document.querySelectorAll("#lanes .srow")].filter((e) => e.getClientRects().length);
  const last = rows.at(-1)?.getBoundingClientRect(), a = all?.getBoundingClientRect(), l = list.getBoundingClientRect();
  return {
    brand: r4(document.querySelector(".brandrow")), search: r4(document.querySelector(".side-search")), nav: r4(document.querySelector("#nav")),
    sidebarTop: sb.scrollTop, listTop: Math.round(list.scrollTop), overflowing: list.scrollHeight > list.clientHeight + 1, atEnd: list.scrollTop + list.clientHeight >= list.scrollHeight - 1,
    footShown: !!all && all.getClientRects().length > 0, footTop: a ? Math.round(a.top * 10) / 10 : null, footBottom: a ? Math.round(a.bottom * 10) / 10 : null,
    listBottom: Math.round(l.bottom * 10) / 10, lastRowBottom: last ? Math.round(last.bottom * 10) / 10 : null, vh: innerHeight,
  };
});
const scrollList = (page, where) => page.evaluate((where) => { const l = document.querySelector("#side-list"); l.scrollTop = where === "end" ? l.scrollHeight : where === "mid" ? (l.scrollHeight - l.clientHeight) / 2 : 0; }, where);

// The contrast ratio of each visible element's text against what is behind it (its ancestors' backgrounds, composited).
const contrast = (page, selector, pseudo) => page.evaluate(({ selector, pseudo }) => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const backdrop = (node) => { const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor)); let c = [255, 255, 255, 1]; for (const layer of layers.reverse()) c = over(layer, c); return c; };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  return [...document.querySelectorAll(selector)].filter((n) => n.getClientRects().length).map((n) => {
    const fg = rgba(getComputedStyle(n, pseudo || null).color), bg = backdrop(n);
    return { text: (n.textContent || n.placeholder || "").trim().slice(0, 30), ratio: Math.round(ratio(fg, bg) * 100) / 100 };
  });
}, { selector, pseudo });

// The sheet: its box, its headings and rows, what scrolls, and what pokes past the screen's edge.
const sheetOf = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.kids-sheet");
  if (!d || !d.open) return null;
  const box = d.getBoundingClientRect(), vw = document.documentElement.clientWidth;
  const rows = [...d.querySelectorAll(".kids-row")], scrollers = [d, ...d.querySelectorAll("*")].filter((e) => /auto|scroll/.test(getComputedStyle(e).overflowY));
  const close = d.querySelector(".vclose").getBoundingClientRect();
  return {
    box: { left: Math.round(box.left), right: Math.round(box.right), bottom: Math.round(box.bottom), width: Math.round(box.width), vh: innerHeight },
    title: d.querySelector(".vt")?.textContent, count: d.querySelector(".vm")?.textContent,
    headings: [...d.querySelectorAll(".kids-h")].map((h) => h.textContent),
    sections: [...d.querySelectorAll(".kids-sec")].map((sec) => [...sec.querySelectorAll(".kids-row")].map((r) => r.dataset.id)),
    ids: rows.map((r) => r.dataset.id), names: rows.map((r) => r.querySelector(".nm").textContent), unders: rows.map((r) => r.querySelector(".under")?.textContent ?? null),
    minRow: rows.length ? Math.round(Math.min(...rows.map((r) => r.getBoundingClientRect().height)) * 10) / 10 : 0,
    search: { height: Math.round((d.querySelector(".kids-search")?.getBoundingClientRect().height ?? 0) * 10) / 10, type: d.querySelector(".kids-search input")?.type ?? null },
    close: Math.round(Math.min(close.width, close.height)),
    scrollers: scrollers.map((e) => e.className || e.tagName), sideways: [...d.querySelectorAll("*")].filter((e) => { const r = e.getBoundingClientRect(); return r.width && (r.right > vw + 0.5 || r.left < -0.5); }).length,
    scrollW: d.scrollWidth <= d.clientWidth, empty: d.querySelector(".empty")?.textContent ?? null,
  };
});

// Sticky rows stick to the top of the sidebar's one scroller, #side-list (the header, search and nav sit above it).
const stickyBox = (page, id) => page.evaluate((id) => {
  const sb = document.querySelector("#side-list"), item = [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id), row = item?.querySelector(":scope > .tree-row");
  if (!row) return null;
  const s = sb.getBoundingClientRect(), r = row.getBoundingClientRect(), f = row.querySelector(".tree-fewer")?.getBoundingClientRect();
  return { top: Math.round((r.top - s.top) * 10) / 10, bottom: Math.round((r.bottom - s.top) * 10) / 10, scrollTop: Math.round(sb.scrollTop), max: Math.round(sb.scrollHeight - sb.clientHeight), stuck: row.classList.contains("stuck"), fewerVisible: !!f && f.top >= s.top - 0.5 && f.bottom <= s.bottom + 0.5, viewport: Math.round(s.height) };
}, id);
// A page whose model is patched on the way in: `patch(model)` runs on every full model the page fetches, and setting `state.edit` to a
// function makes the next poll return the model changed by it (a live update), once.
const servedModel = async (browser, opts, patch) => {
  const page = await served(browser, opts), state = { edit: null, version: null, n: 0 };
  await page.route((u) => u.pathname === "/api/model", async (route) => {
    const url = new URL(route.request().url()), since = url.searchParams.get("since");
    if (since !== null && !state.edit) return state.version && since === state.version ? route.fulfill({ status: 304 }) : route.continue();
    url.searchParams.delete("since");
    const res = await route.fetch({ url: url.toString() }), m = patch(await res.json());
    if (since !== null) { state.edit(m); m.version = state.version = "live-" + (++state.n); state.edit = null; }
    await route.fulfill({ status: 200, headers: { "content-type": "application/json" }, body: JSON.stringify(m) });
  });
  await page.reload({ waitUntil: "load" }); await settled(page);
  return { page, state };
};
const scrollSidebar = async (page, y) => { await page.evaluate((y) => { document.querySelector("#side-list").scrollTop = y; }, y); await page.waitForTimeout(80); };

export default async function sidebarCheck(browser) {
  const D = await data({ extras: true });
  const r = reporter("sidebar");
  const R = r.results;
  const fan = Object.values(D.SESS).find((s) => s.name === "Fan-out"), swarm = Object.values(D.SESS).find((s) => s.name === "Swarm");
  r.expect(!!fan && !!swarm, "the extras fixture has no Fan-out or no Swarm session");
  if (!fan || !swarm) return r.done();
  const kidsOf = (parent) => D.H.filter((h) => h.kind === "spawn" && h.from === parent.id).map((h) => D.SESS[h.to]).filter(Boolean).sort((a, b) => b.last - a.last);
  const name = (id) => D.SESS[id]?.name ?? id;
  const rankOf = (c) => (c.state === "wait" ? 0 : c.state === "work" ? 1 : 2);
  // Waiting, then running, then finished, newest first in each; the short list is the running ones (at most 8), then the newest finished until three rows.
  const ordered = (ks) => [...ks].sort((a, b) => rankOf(a) - rankOf(b) || b.last - a.last);
  const shortList = (ks) => { const sorted = ordered(ks), keep = new Set(); for (const c of sorted) if (rankOf(c) < 2 && keep.size < 8) keep.add(c.id); for (const c of sorted) if (keep.size < 3) keep.add(c.id); return sorted.filter((c) => keep.has(c.id)); };
  const fanKids = kidsOf(fan), swarmKids = kidsOf(swarm);
  const fanShort = shortList(fanKids).map((c) => c.id), swarmShort = shortList(swarmKids).map((c) => c.id);
  const fanFull = ordered(fanKids).map((c) => c.id), swarmFull = ordered(swarmKids).map((c) => c.id);
  const fanNames = ["Reader 1", "Reader 7", "Reader 6"], swarmNames = ["Worker 10", "Worker 9", "Worker 8", "Worker 7", "Worker 6", "Worker 5", "Worker 4", "Worker 3"];
  r.expect(fanKids.length === 7 && fanKids.filter((c) => rankOf(c) < 2).length === 1 && rankOf(fanKids.at(-1)) === 1, "Fan-out needs 7 children, the oldest the only running one: states " + fanKids.map((c) => c.state).join(","));
  r.expect(swarmKids.length === 12 && swarmKids.filter((c) => c.state === "work").length === 10, "Swarm needs 12 children, 10 of them running: states " + swarmKids.map((c) => c.state).join(","));
  r.expect(JSON.stringify(fanShort.map(name)) === JSON.stringify(fanNames), "Fan-out's expected short list is Reader 1 then the two newest finished: " + JSON.stringify(fanShort.map(name)));
  r.expect(JSON.stringify(swarmShort.map(name)) === JSON.stringify(swarmNames), "Swarm's expected short list is its eight newest running workers: " + JSON.stringify(swarmShort.map(name)));
  R.expected = { fan: fanShort.map(name), swarm: swarmShort.map(name), fanKids: fanKids.length, swarmKids: swarmKids.length };

  // The same walk the viewer does: a session's children are those it spawned.
  const kidMap = new Map();
  for (const s of Object.values(D.SESS)) { const p = s.parent ?? D.H.find((h) => h.kind === "spawn" && h.to === s.id)?.from; if (p && D.SESS[p]) { if (!kidMap.has(p)) kidMap.set(p, []); kidMap.get(p).push(s); } }
  const below = (id, seen = new Set([id])) => (kidMap.get(id) ?? []).flatMap((k) => (seen.has(k.id) ? [] : (seen.add(k.id), [k, ...below(k.id, seen)])));

  // ---- Phone, light: the short lists, the "All N" row, prefs, search, keys, the sheet ------------------------------------
  {
    const page = await served(browser, { extras: true, size: "phone", dark: false });
    await openDrawer(page);
    const first = await groupOf(page, fan.id), second = await groupOf(page, swarm.id);
    R.phoneFirst = first; R.phoneSwarm = second;
    r.expect(first && JSON.stringify(first.ids) === JSON.stringify(fanShort), "phone: Fan-out lists its running child, then the newest finished: " + JSON.stringify(first?.ids.map(name)) + " expected " + JSON.stringify(fanNames));
    r.expect(first?.expanded === "true", "phone: Fan-out is open by default (a child is running)");
    r.expect(first?.all?.text === "All 7" && first.all.tag === "BUTTON" && first.all.tabIndex === 0 && first.all.role === "treeitem", "phone: one focusable 'All 7' row: " + JSON.stringify(first?.all));
    r.expect(first?.all?.label === "All 7 sessions under Fan-out", "phone: the row's aria-label names the parent: " + first?.all?.label);
    r.expect(first?.pill === "7", "phone: N is the count pill's number: pill " + first?.pill);
    r.expect(first?.all && first.all.height >= 44, "phone: the 'All N' row is at least 44 px tall: " + first?.all?.height);
    r.expect(first?.oldButtons === 0, "phone: no 'Show more' button is left");
    r.expect(first?.stored === null, "phone: nothing is saved until a parent is toggled: " + JSON.stringify(first?.stored));
    r.expect(second && JSON.stringify(second.ids) === JSON.stringify(swarmShort), "phone: Swarm lists its eight newest running workers and no finished one: " + JSON.stringify(second?.ids.map(name)));
    r.expect(second?.all?.text === "All 12" && second.pill === "12", "phone: Swarm ends with 'All 12', its pill's number: " + JSON.stringify([second?.all?.text, second?.pill]));
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light.png") });

    // Defaults: a parent is open only when something below it is waiting or running (nothing is saved yet, and no session is open).
    const defaults = await page.evaluate(() => [...document.querySelectorAll("#lanes > .treeitem[aria-expanded]")].map((x) => ({ id: x.dataset.id, expanded: x.getAttribute("aria-expanded"), pill: x.querySelector(":scope > .tree-row .tree-summary")?.textContent.trim() ?? null })));
    R.defaults = defaults.map((d) => ({ name: name(d.id), expanded: d.expanded, pill: d.pill }));
    for (const d of defaults) {
      const active = below(d.id).some((k) => k.state === "work" || k.state === "wait");
      r.expect(d.expanded === String(active), "phone: " + name(d.id) + " is " + (d.expanded === "true" ? "open" : "collapsed") + " by default but " + (active ? "has" : "has no") + " running or waiting session below it");
      if (!active) r.expect(d.pill === String(below(d.id).length), "phone: the collapsed " + name(d.id) + " shows its count pill, " + below(d.id).length + ": " + d.pill);
    }
    r.expect(defaults.some((d) => d.expanded === "false"), "phone: the fixture shows a collapsed parent, so the default-collapsed check proves nothing: " + JSON.stringify(R.defaults));

    const box = await toggleBox(page, fan.id);
    R.phoneToggle = box;
    r.expect(box && box.w >= 44 && box.h >= 44, "phone: the toggle is at least 44x44: " + JSON.stringify(box));
    r.expect(box?.right && box.lower, "phone: the toggle sits over the right end of the row's meta line: " + JSON.stringify(box));
    r.expect(box?.color === "rgb(93, 101, 97)", "phone: the chevron is --muted (5.3:1 on the light sidebar): " + box?.color);
    r.expect(box?.bg === "rgba(0, 0, 0, 0)", "phone: the toggle has no background until hover or focus: " + box?.bg);
    const g = await gutters(page);
    R.phoneGutters = g;
    r.expect(g.spacers === 0, "phone: no .tree-spacer remains");
    r.expect(g.mixed >= 1, "phone: no group mixes rows with and without children, so the gutter check proves nothing");
    r.expect(g.groups.every((x) => x.lefts.length === 1), "phone: names in a group start at different x: " + JSON.stringify(g.groups));
    r.expect(g.groups.every((x) => x.ends.length === 1), "phone: times in a group end at different x: " + JSON.stringify(g.groups));
    const cut = await page.evaluate(() => [...document.querySelectorAll("#lanes .srow-meta .repo-short")].filter((e) => e.getClientRects().length && e.scrollWidth > e.clientWidth + 1).map((e) => e.textContent + " " + e.clientWidth + "/" + e.scrollWidth));
    R.phoneRepoCut = cut;
    r.expect(cut.length === 0, "phone: repo names are cut on the meta line: " + cut.join(", "));
    const allContrast = await contrast(page, "#lanes .tree-all");
    R.phoneAllContrast = allContrast;
    r.expect(allContrast.length >= 2 && allContrast.every((c) => c.ratio >= 4.5), "phone: the 'All N' rows meet AA contrast (4.5): " + JSON.stringify(allContrast));

    // Saved values from before: {open, more} keeps its open and ignores `more`; an entry with only `more` is dropped.
    await page.evaluate(({ fan, swarm }) => { localStorage.setItem("semon.tree", JSON.stringify({ [fan]: { open: true, more: true, at: 5 }, [swarm]: { more: true, at: 6 } })); }, { fan: fan.id, swarm: swarm.id });
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#lanes .treeitem"));
    const legacy = await groupOf(page, fan.id), legacy2 = await groupOf(page, swarm.id);
    R.phoneLegacy = { fan: legacy, swarm: legacy2 };
    r.expect(legacy && legacy.expanded === "true" && JSON.stringify(legacy.ids) === JSON.stringify(fanShort) && legacy.all?.text === "All 7", "phone: an older saved {open, more} still loads and its `more` reveals nothing: " + JSON.stringify(legacy));
    r.expect(legacy2 && JSON.stringify(legacy2.ids) === JSON.stringify(swarmShort), "phone: an older entry with only `more` changes nothing: " + JSON.stringify(legacy2?.ids.map(name)));
    // The next save writes the pruned prefs: Swarm's toggle saves its own choice, and Fan-out's entry loses its `more`.
    await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), swarm.id);
    const pruned = await page.evaluate(() => JSON.parse(localStorage.getItem("semon.tree") ?? "{}"));
    R.phonePruned = pruned;
    r.expect(pruned[swarm.id]?.open === false && pruned[fan.id]?.open === true && Object.values(pruned).every((v) => !("more" in v)), "phone: the next save drops every `more`: " + JSON.stringify(pruned));
    await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), swarm.id);
    await openDrawer(page);

    // Search: the children that match are the pool, capped by the same rule, and "All N" stays when others are hidden.
    await page.fill("#q", "Reader 3"); await page.waitForTimeout(120);
    const one = await groupOf(page, fan.id);
    R.phoneSearchOne = one;
    r.expect(one && one.ids.length === 1 && name(one.ids[0]) === "Reader 3" && one.all?.text === "All 7", "phone: a search lists only the child that matches, and keeps 'All 7': " + JSON.stringify(one?.ids.map(name)) + " " + one?.all?.text);
    await page.fill("#q", "Reader"); await page.waitForTimeout(120);
    const every = await groupOf(page, fan.id);
    r.expect(every && JSON.stringify(every.ids) === JSON.stringify(fanShort) && every.all?.text === "All 7", "phone: a search that matches every child is capped like any list, with 'All 7': " + JSON.stringify(every?.ids.map(name)) + " " + every?.all?.text);
    await page.fill("#q", ""); await page.waitForTimeout(120);
    const cleared = await groupOf(page, fan.id);
    r.expect(cleared && JSON.stringify(cleared.ids) === JSON.stringify(fanShort) && cleared.all, "phone: clearing the search lists the short list again");

    // Keys: Tab from the last listed child reaches "All N"; the arrows act on their own row.
    await page.evaluate((id) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id); item.querySelector(":scope > .tree-group > .treeitem:last-of-type .srow").focus(); }, fan.id);
    await page.keyboard.press("Tab");
    const tabbed = await page.evaluate(() => ({ cls: document.activeElement?.className, id: document.activeElement?.dataset?.id }));
    R.phoneTab = tabbed;
    r.expect(tabbed.cls === "tree-all" && tabbed.id === fan.id, "phone: Tab from the last child reaches 'All N': " + JSON.stringify(tabbed));
    await page.keyboard.press("ArrowLeft");
    const arrowAll = await groupOf(page, fan.id);
    r.expect(arrowAll?.expanded === "true" && arrowAll.stored?.open !== false, "phone: ArrowLeft on 'All N' leaves the parent open: " + JSON.stringify(arrowAll));
    await page.evaluate((id) => [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id).focus(), fanShort[0]);
    await page.keyboard.press("ArrowLeft");
    const arrowLeaf = await groupOf(page, fan.id);
    r.expect(arrowLeaf?.expanded === "true" && arrowLeaf.stored?.open !== false, "phone: ArrowLeft on a leaf leaves its parent open: " + JSON.stringify(arrowLeaf));

    // The sheet, from Fan-out's "All 7".
    await page.click('#lanes .tree-all[data-id="' + fan.id + '"]');
    await page.waitForSelector("dialog.kids-sheet[open]");
    await page.waitForTimeout(150);
    let sheet = await sheetOf(page);
    R.phoneSheet = sheet;
    r.expect(sheet && sheet.box.left === 0 && sheet.box.width === 390 && Math.abs(sheet.box.bottom - sheet.box.vh) <= 1, "phone: the sheet is a bottom sheet at the screen's foot: " + JSON.stringify(sheet?.box));
    r.expect(sheet?.title === "Fan-out" && sheet.count === "7 sessions", "phone: the sheet names the parent and counts its sessions: " + JSON.stringify([sheet?.title, sheet?.count]));
    r.expect(JSON.stringify(sheet?.headings) === JSON.stringify(["Running (1)", "Finished (6)"]), "phone: the sections are 'Running (1)' and 'Finished (6)', with the empty one omitted: " + JSON.stringify(sheet?.headings));
    r.expect(JSON.stringify(sheet?.ids) === JSON.stringify(fanFull), "phone: the sheet lists exactly the descendants, running first, newest first: " + JSON.stringify(sheet?.names));
    r.expect(sheet && sheet.minRow >= 44 && sheet.search.height >= 44 && sheet.close >= 44, "phone: rows, search and close are at least 44 px: " + JSON.stringify([sheet?.minRow, sheet?.search, sheet?.close]));
    r.expect(sheet?.search.type === "search", "phone: the sheet has a search field");
    r.expect(sheet && sheet.scrollers.length === 1 && sheet.scrollers[0] === "vb", "phone: one scroll surface: " + JSON.stringify(sheet?.scrollers));
    r.expect(sheet && sheet.sideways === 0 && sheet.scrollW, "phone: no sideways overflow in the sheet: " + JSON.stringify([sheet?.sideways, sheet?.scrollW]));
    const sheetContrast = [...await contrast(page, "dialog.kids-sheet .kids-h, dialog.kids-sheet .kids-row .nm, dialog.kids-sheet .kids-row .ag, dialog.kids-sheet .vt span, dialog.kids-sheet .vm"), ...await contrast(page, "dialog.kids-sheet .kids-search input", "::placeholder")];
    R.phoneSheetContrast = sheetContrast;
    r.expect(sheetContrast.length >= 10 && sheetContrast.every((c) => c.ratio >= 4.5), "phone: the sheet's text meets AA contrast (4.5): " + JSON.stringify(sheetContrast.filter((c) => c.ratio < 4.5)));
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light-sheet.png") });

    // Search filters it.
    await page.fill("dialog.kids-sheet .kids-search input", "Reader 3"); await page.waitForTimeout(80);
    sheet = await sheetOf(page);
    r.expect(sheet && sheet.names.length === 1 && sheet.names[0] === "Reader 3" && JSON.stringify(sheet.headings) === JSON.stringify(["Finished (1)"]), "phone: the search narrows the sheet to Reader 3: " + JSON.stringify([sheet?.names, sheet?.headings]));
    await page.fill("dialog.kids-sheet .kids-search input", "no such session"); await page.waitForTimeout(80);
    sheet = await sheetOf(page);
    r.expect(sheet && sheet.ids.length === 0 && sheet.headings.length === 0 && sheet.empty?.startsWith("No sessions match"), "phone: a search with no match says so and shows no heading: " + JSON.stringify([sheet?.ids, sheet?.empty]));
    await page.fill("dialog.kids-sheet .kids-search input", ""); await page.waitForTimeout(80);
    sheet = await sheetOf(page);
    r.expect(sheet && sheet.ids.length === 7, "phone: clearing the search lists every session again: " + sheet?.ids.length);

    // A tap on a row opens that session and closes the sheet.
    const target = fanFull[3];
    await page.click('dialog.kids-sheet .kids-row[data-id="' + target + '"]');
    await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
    await page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, name(target));
    r.expect(await page.evaluate(() => !document.querySelector("dialog.kids-sheet") && location.pathname.includes("/s/")), "phone: the sheet closes and the tapped session opens: " + await page.evaluate(() => location.pathname));
    r.expect(await overflow(page) === 0, "phone: no sideways overflow after the sheet");

    // Esc closes it and returns focus to "All N"; so does a tap on the backdrop.
    for (const how of ["Escape", "backdrop"]) {
      await goto(page, { v: "sessions" }, D);
      await openDrawer(page);
      await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
      await page.waitForSelector("dialog.kids-sheet[open]");
      const swarmSheet = await sheetOf(page);
      if (how === "Escape") {
        R.phoneSwarmSheet = swarmSheet;
        r.expect(swarmSheet && JSON.stringify(swarmSheet.ids) === JSON.stringify(swarmFull) && JSON.stringify(swarmSheet.headings) === JSON.stringify(["Running (10)", "Finished (2)"]), "phone: Swarm's sheet lists all twelve under 'Running (10)' and 'Finished (2)': " + JSON.stringify([swarmSheet?.headings, swarmSheet?.names]));
        await page.keyboard.press("Escape");
      } else await page.mouse.click(195, 30);
      await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
      const back = await page.evaluate(() => ({ cls: document.activeElement?.className, id: document.activeElement?.dataset?.id, drawer: document.body.classList.contains("drawer-open") }));
      r.expect(back.cls === "tree-all" && back.id === swarm.id, "phone: " + how + " closes the sheet and returns focus to 'All N': " + JSON.stringify(back));
      r.expect(back.drawer === true, "phone: " + how + " closes only the sheet, the drawer stays open: " + JSON.stringify(back));
      await page.waitForTimeout(400);
      r.expect(await page.evaluate(() => document.body.classList.contains("drawer-open") && getComputedStyle(document.querySelector("#sidebar")).visibility === "visible"), "phone: " + how + ": the drawer is still open and visible after its transition");
    }
    r.expect(await overflow(page) === 0, "phone: the sidebar has no sideways overflow");
    r.expect(page.errors.length === 0, "phone: page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- Phone, dark: screenshots and contrast --------------------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, size: "phone", dark: true });
    await openDrawer(page);
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-dark.png") });
    await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
    await page.waitForSelector("dialog.kids-sheet[open]"); await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-dark-sheet.png") });
    const dark = [...await contrast(page, "dialog.kids-sheet .kids-h, dialog.kids-sheet .kids-row .nm, dialog.kids-sheet .kids-row .ag, dialog.kids-sheet .vt span, dialog.kids-sheet .vm"), ...await contrast(page, "dialog.kids-sheet .kids-search input", "::placeholder")];
    R.phoneDarkContrast = dark;
    r.expect(dark.length >= 10 && dark.every((c) => c.ratio >= 4.5), "phone dark: the sheet's text meets AA contrast (4.5): " + JSON.stringify(dark.filter((c) => c.ratio < 4.5)));
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
    const allDark = await contrast(page, "#lanes .tree-all");
    r.expect(allDark.length >= 2 && allDark.every((c) => c.ratio >= 4.5), "phone dark: the 'All N' rows meet AA contrast (4.5): " + JSON.stringify(allDark));
    r.expect(page.errors.length === 0, "phone dark: page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- Desktop: the toggle, the short lists, the whole list inline with its sticky row ----------------------------------------
  for (const dark of [false, true]) {
    const page = await served(browser, { extras: true, size: "desktop", dark });
    const tag = dark ? "dark" : "light";
    await page.waitForSelector("#lanes .treeitem");
    if (!dark) {
      const box = await toggleBox(page, fan.id);
      R.desktopToggle = box;
      r.expect(box && box.w >= 28 && box.h >= 36, "desktop: the toggle's hit area is at least 28x36: " + JSON.stringify(box));
      r.expect(box?.right && box.lower, "desktop: the toggle sits over the right end of the row's meta line: " + JSON.stringify(box));
      const g = await gutters(page);
      R.desktopGutters = g;
      r.expect(g.spacers === 0 && g.mixed >= 1 && g.groups.every((x) => x.lefts.length === 1 && x.ends.length === 1), "desktop: names in a group start at one x and times end at one x: " + JSON.stringify(g));
      // Open, the child count is hidden; collapsed, the count and state dot sit inside the row, clear of the toggle.
      const summaryShown = () => page.evaluate((id) => { const s = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-summary"); return !!s && getComputedStyle(s).display !== "none"; }, fan.id);
      r.expect(!(await summaryShown()), "desktop: an open parent shows no child count");
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
      r.expect(await summaryShown(), "desktop: collapsing shows the child count at once, without a re-render");
      const collapsed = await page.evaluate((id) => {
        const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id), s = item.querySelector(":scope > .tree-row .tree-summary"), t = item.querySelector(":scope > .tree-row .tree-toggle");
        if (!s || !t) return null;
        const a = s.getBoundingClientRect(), b = t.getBoundingClientRect();
        return { expanded: item.getAttribute("aria-expanded"), summary: s.textContent, gap: Math.round((b.left - a.right) * 10) / 10 };
      }, fan.id);
      R.desktopCollapsed = collapsed;
      r.expect(collapsed?.expanded === "false" && collapsed.summary === String(fanKids.length) && collapsed.gap >= 0, "desktop: the collapsed summary clears the toggle: " + JSON.stringify(collapsed));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-light-collapsed.png") });
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
      await page.waitForTimeout(300);
      r.expect(!(await summaryShown()), "desktop: expanding hides the child count again");
    }
    const first = await groupOf(page, fan.id), second = await groupOf(page, swarm.id);
    R.desktopFirst = { fan: first, swarm: second };
    r.expect(first && JSON.stringify(first.ids) === JSON.stringify(fanShort) && first.all?.text === "All 7" && first.pill === "7", "desktop " + tag + ": Fan-out's short list and 'All 7': " + JSON.stringify(first));
    r.expect(second && JSON.stringify(second.ids) === JSON.stringify(swarmShort) && second.all?.text === "All 12", "desktop " + tag + ": Swarm's short list and 'All 12': " + JSON.stringify(second));
    await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-" + tag + ".png") });

    // "All 7" opens the whole list inline: no dialog, the parent's row sticks, and "Show fewer" is focused.
    await page.click('#lanes .tree-all[data-id="' + fan.id + '"]');
    await page.waitForTimeout(150);
    const open = await groupOf(page, fan.id);
    R.desktopInline = open;
    r.expect(open && JSON.stringify(open.ids) === JSON.stringify(fanFull) && !open.all, "desktop " + tag + ": 'All 7' lists all seven inline, running first, newest first, and the row is gone: " + JSON.stringify(open?.ids.map(name)));
    r.expect(open?.dialogs === 0, "desktop " + tag + ": 'All N' opens no dialog");
    r.expect(open?.stuck === true && open.fewer === "Show fewer", "desktop " + tag + ": the parent's row is sticky and carries 'Show fewer': " + JSON.stringify([open?.stuck, open?.fewer]));
    r.expect(await page.evaluate(() => document.activeElement?.classList.contains("tree-fewer")), "desktop " + tag + ": focus moves to 'Show fewer'");
    r.expect(await page.evaluate(() => document.querySelectorAll("#lanes .tree-row.stuck").length) === 1, "desktop " + tag + ": one sticky row");
    await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-" + tag + "-all.png") });
    await page.keyboard.press("Enter");
    await page.waitForTimeout(150);
    const folded = await groupOf(page, fan.id);
    r.expect(folded && JSON.stringify(folded.ids) === JSON.stringify(fanShort) && folded.all?.text === "All 7" && !folded.stuck, "desktop " + tag + ": 'Show fewer' folds the list back to the short one: " + JSON.stringify(folded?.ids.map(name)));
    r.expect(await page.evaluate((id) => document.activeElement?.classList.contains("srow") && document.activeElement.dataset.id === id, fan.id), "desktop " + tag + ": 'Show fewer' leaves focus on the parent");

    // Swarm, in a short window so its whole list scrolls: the parent stays within the top pixel, and folding needs no scroll.
    await page.setViewportSize({ width: 1280, height: 520 });
    await page.waitForTimeout(150);
    const before = await stickyBox(page, swarm.id), natural = before.top + before.scrollTop;
    await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
    await page.waitForTimeout(150);
    const start = await stickyBox(page, swarm.id);
    R.desktopSticky = { natural, start };
    r.expect(start?.max >= natural + 160, "desktop " + tag + ": the expanded list scrolls far enough to test the sticky row: " + JSON.stringify({ natural, start }));
    const opened = await groupOf(page, swarm.id);
    r.expect(opened && JSON.stringify(opened.ids) === JSON.stringify(swarmFull), "desktop " + tag + ": Swarm's whole list is inline, running first, newest first: " + JSON.stringify(opened?.ids.map(name)));
    const seen = [];
    for (const d of [60, 160, 300, 480]) {
      if (natural + d > start.max) continue;
      await scrollSidebar(page, natural + d);
      const at = await stickyBox(page, swarm.id);
      seen.push(at);
      r.expect(at && at.stuck && Math.abs(at.top) <= 1 && at.fewerVisible, "desktop " + tag + ": scrolled to " + at?.scrollTop + " the parent's row is within the sidebar's top pixel with 'Show fewer' visible: " + JSON.stringify(at));
    }
    R.desktopStickySeen = seen;
    r.expect(seen.length >= 2, "desktop " + tag + ": the sticky row was checked at two scroll positions at least: " + seen.length);
    await scrollSidebar(page, natural + 160);
    await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-" + tag + "-sticky.png") });

    // Navigating to a row that sits under the sticky row scrolls it clear of that row.
    const target = swarmFull[2];
    await page.evaluate((id) => { const sb = document.querySelector("#side-list"), row = sb.querySelector('.srow[data-id="' + CSS.escape(id) + '"]'); sb.scrollTop += row.getBoundingClientRect().top - sb.getBoundingClientRect().top - 4; }, target);
    await page.waitForTimeout(80);
    await page.evaluate((id) => document.querySelector('#lanes .srow[data-id="' + CSS.escape(id) + '"]').click(), target);
    await page.waitForFunction((t) => document.querySelector("#topbar .t")?.textContent === t, name(target));
    await page.waitForTimeout(150);
    const cleared2 = await page.evaluate(() => {
      const sb = document.querySelector("#side-list").getBoundingClientRect(), cur = document.querySelector('#lanes .srow[aria-current="page"]')?.getBoundingClientRect(), st = document.querySelector("#lanes .tree-row.stuck")?.getBoundingClientRect();
      return cur && st ? { curTop: Math.round(cur.top - sb.top), curBottom: Math.round(cur.bottom - sb.top), stickyBottom: Math.round(st.bottom - sb.top), viewport: Math.round(sb.height) } : null;
    });
    R.desktopReveal = cleared2;
    r.expect(cleared2 && cleared2.curTop >= cleared2.stickyBottom - 1 && cleared2.curBottom <= cleared2.viewport + 1, "desktop " + tag + ": the open session's row is scrolled clear of the sticky row: " + JSON.stringify(cleared2));

    // "Show fewer" on the sticky row folds it, brings the parent into view and focuses it.
    await page.click("#lanes .tree-row.stuck .tree-fewer");
    await page.waitForTimeout(150);
    const back = await groupOf(page, swarm.id), where = await stickyBox(page, swarm.id);
    R.desktopFolded = { back, where };
    r.expect(back && JSON.stringify(back.ids) === JSON.stringify(swarmShort) && back.all?.text === "All 12" && !back.stuck, "desktop " + tag + ": 'Show fewer' restores the short list: " + JSON.stringify(back?.ids.map(name)));
    r.expect(where && where.top >= -1 && where.bottom <= where.viewport + 1, "desktop " + tag + ": the parent's row is back in the sidebar's view: " + JSON.stringify(where));
    r.expect(await page.evaluate((id) => document.activeElement?.classList.contains("srow") && document.activeElement.dataset.id === id, swarm.id), "desktop " + tag + ": focus is on the parent's row");

    // The open list is not saved: a reload starts short. Collapsing the parent also folds it.
    await goto(page, { v: "sessions" }, D);
    await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
    await page.waitForTimeout(100);
    r.expect((await groupOf(page, swarm.id))?.stuck === true, "desktop " + tag + ": the list opens again");
    const stored = await page.evaluate(() => JSON.parse(localStorage.getItem("semon.tree") ?? "{}"));
    r.expect(Object.values(stored).every((v) => Object.keys(v).every((k) => k === "open" || k === "at")), "desktop " + tag + ": nothing about the open list is saved: " + JSON.stringify(stored));
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#lanes .treeitem"));
    const reloaded = await groupOf(page, swarm.id);
    r.expect(reloaded && JSON.stringify(reloaded.ids) === JSON.stringify(swarmShort) && !reloaded.stuck && reloaded.all?.text === "All 12", "desktop " + tag + ": a reload starts with the short list: " + JSON.stringify(reloaded?.ids.map(name)));
    await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
    await page.waitForTimeout(100);
    await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), swarm.id);
    await page.waitForTimeout(100);
    const collapsedAfter = await groupOf(page, swarm.id);
    r.expect(collapsedAfter && collapsedAfter.expanded === "false" && !collapsedAfter.stuck, "desktop " + tag + ": collapsing the parent drops its sticky row: " + JSON.stringify([collapsedAfter?.expanded, collapsedAfter?.stuck]));
    // The count pill: a neutral total with no state dot (a dot next to "53" read as 53 running); it turns amber only when a
    // descendant needs you; its tooltip and the row's accessible label break the total down, failures included. It collapses every
    // parent in turn and saves that, so it runs after the reload assertions.
    {
      const pills = await collapsedPills(page);
      (R.desktopPill ??= {})[tag] = pills;
      assertPills(r, "desktop " + tag, pills, D.SESS, D.H, ["Fan-out"]);
    }
    r.expect(page.errors.length === 0, "desktop " + tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- The count pill with a waiting and a failed run inside (a patched model), desktop and phone -------------------------
  {
    const kidIds = fanKids.map((c) => c.id), M = patchModel(structuredClone(D.model), kidIds), SESS = M.sessions;
    for (const [size, dark] of [["desktop", false], ["desktop", true], ["phone", false], ["phone", true]]) {
      const tag = size + (dark ? "-dark" : "-light"), page = await servedPatched(browser, { extras: true, size, dark }, kidIds);
      if (size === "phone") await openDrawer(page);
      await page.waitForSelector("#lanes .treeitem");
      // The short list with a waiting run and a failed one in it: the waiting run first, then the running one, then the newest finished.
      const patchedKids = Object.values(SESS).filter((x) => x.parent === fan.id), patchedShort = shortList(patchedKids).map((c) => c.id), pt = tally(SESS, M.handoffs, fan.id);
      const patchedFan = await groupOf(page, fan.id);
      R.patchedShort = { ids: patchedShort.map((id) => SESS[id].name), got: patchedFan?.ids.map((id) => SESS[id]?.name) };
      r.expect(patchedFan && JSON.stringify(patchedFan.ids) === JSON.stringify(patchedShort) && SESS[patchedShort[0]].state === "wait" && SESS[patchedShort[1]].state === "work" && patchedFan.all?.text === "All " + pt.total, "patched " + tag + ": the short list puts the waiting run first, then the running one, and 'All " + pt.total + "': " + JSON.stringify([patchedFan?.ids.map((id) => SESS[id]?.name), patchedFan?.all?.text]));
      if (size === "phone") {
        await page.click('#lanes .tree-all[data-id="' + fan.id + '"]');
        await page.waitForSelector("dialog.kids-sheet[open]");
        await page.waitForTimeout(150);
        const sheet = await sheetOf(page), heads = ["Waiting for you (" + pt.wait + ")", "Running (" + pt.work + ")", "Finished (" + (pt.total - pt.wait - pt.work) + ")"];
        r.expect(sheet && JSON.stringify(sheet.headings) === JSON.stringify(heads) && sheet.ids.length === pt.total && sheet.unders.filter(Boolean).length === 1 && sheet.unders.filter(Boolean)[0] === "under " + SESS[fanKids[3].id].name, "patched " + tag + ": the sheet has a waiting section first and names the grandchild's parent: " + JSON.stringify([sheet?.headings, sheet?.unders?.filter(Boolean)]));
        r.expect(sheet && sheet.sections?.[0]?.[0] === patchedShort[0], "patched " + tag + ": the waiting run is the sheet's first row");
        await page.screenshot({ path: path.join(ENV.out, "sidebar-wait-sheet-" + tag + ".png") });
        await page.keyboard.press("Escape");
        await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
      }
      const pills = await collapsedPills(page);
      (R.patchedPill ??= {})[tag] = pills.filter((p) => SESS[p.id]?.name === "Fan-out");
      assertPills(r, "patched " + tag, pills, SESS, M.handoffs, ["Fan-out"]);
      const fanPill = pills.find((p) => p.id === fan.id), t = tally(SESS, M.handoffs, fan.id);
      r.expect(t.wait === 1 && t.err === 1 && t.total === fanKids.length + 1, "patched " + tag + ": the patch did not land: " + JSON.stringify(t));
      r.expect(fanPill?.wait === true && /1 needs you/.test(fanPill.tip) && /1 failed/.test(fanPill.tip), "patched " + tag + ": Fan-out's pill is amber and names the waiting and the failed run: " + JSON.stringify(fanPill));
      // Collapsed for the screenshot.
      await page.evaluate((id) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id); if (item.getAttribute("aria-expanded") === "true") item.querySelector(":scope > .tree-row .tree-toggle").click(); }, fan.id);
      await page.screenshot({ path: path.join(ENV.out, "sidebar-wait-pill-" + tag + ".png") });
      await page.context().close();
    }
  }

  // ---- Nested parents, a live update, and a change of screen ------------------------------------------------------------------
  {
    const c1 = fanShort[0], source = fanFull.at(-1);
    // Reader 1 (running, so listed) gets five sessions of its own: one running, four finished. Fan-out then has 12 descendants.
    const nest = (m) => {
      if (m.sessions["nest-1"]) return m;
      for (let i = 1; i <= 5; i++) m.sessions["nest-" + i] = { ...structuredClone(m.sessions[source]), id: "nest-" + i, name: "Helper " + i, parent: c1, state: i === 1 ? "work" : "done", last: m.sessions[source].last - i * 60000 };
      return m;
    };
    const NM = nest(structuredClone(D.model)), nname = (id) => NM.sessions[id]?.name ?? name(id);
    const nestKids = ["nest-1", "nest-2", "nest-3", "nest-4", "nest-5"], nestShort = ["nest-1", "nest-2", "nest-3"];
    const finishSwarm = (m) => {
      const w10 = swarmKids.find((c) => c.name === "Worker 10"), w13 = "swarm-worker-13";
      m.sessions[w10.id].state = "done";
      m.sessions[w13] = { ...structuredClone(m.sessions[w10.id]), id: w13, name: "Worker 13", state: "work", last: m.sessions[w10.id].last + 3 * 60000 };
      return m;
    };
    const live = (() => { const m = finishSwarm(nest(structuredClone(D.model))), ks = Object.values(m.sessions).filter((x) => x.parent === swarm.id); return ordered(ks).map((x) => x.id); })();

    for (const dark of [false, true]) {
      const tag = dark ? "dark" : "light", { page, state } = await servedModel(browser, { extras: true, size: "desktop", dark }, nest);
      await page.waitForSelector("#lanes .treeitem");
      const start = await groupOf(page, fan.id), inner = await groupOf(page, c1);
      R["nested" + tag] = { start, inner };
      r.expect(start && JSON.stringify(start.ids) === JSON.stringify(fanShort) && start.all?.text === "All 12" && start.pill === "12", "nested " + tag + ": Fan-out counts its 12 descendants: " + JSON.stringify([start?.ids.map(nname), start?.all?.text, start?.pill]));
      r.expect(inner && JSON.stringify(inner.ids) === JSON.stringify(nestShort) && inner.all?.text === "All 5" && inner.expanded === "true", "nested " + tag + ": Reader 1 lists its running helper and the two newest finished, and 'All 5': " + JSON.stringify([inner?.ids.map(nname), inner?.all?.text, inner?.expanded]));

      // A parent's "All N" reveals exactly N: every descendant, under its own parent, all open.
      await page.click('#lanes .tree-all[data-id="' + fan.id + '"]');
      await page.waitForTimeout(150);
      const full = await page.evaluate((id) => {
        const item = [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id), rows = [...item.querySelectorAll(":scope > .tree-group .srow")];
        return { rows: rows.length, visible: rows.filter((x) => x.getClientRects().length).length, alls: item.querySelectorAll(".tree-all").length, stuck: [...document.querySelectorAll("#lanes .tree-row.stuck")].map((x) => x.closest(".treeitem").dataset.id) };
      }, fan.id);
      R["nestedFull" + tag] = full;
      r.expect(full.rows === 12 && full.visible === 12 && full.alls === 0, "nested " + tag + ": Fan-out's 'All 12' shows 12 rows, all visible, and no nested 'All N': " + JSON.stringify(full));
      r.expect(full.stuck.length === 1 && full.stuck[0] === fan.id, "nested " + tag + ": only Fan-out's row is sticky: " + JSON.stringify(full.stuck));
      const under = await groupOf(page, c1);
      r.expect(under && JSON.stringify(under.ids) === JSON.stringify(nestKids), "nested " + tag + ": Reader 1's five helpers show under Reader 1: " + JSON.stringify(under?.ids.map(nname)));
      await page.click("#lanes .tree-row.stuck .tree-fewer");
      await page.waitForTimeout(120);

      // Opening the nested "All 5" leaves Fan-out and Reader 1 listed and sticks only Reader 1's row.
      await page.setViewportSize({ width: 1280, height: 520 });
      await page.waitForTimeout(150);
      const before = await stickyBox(page, c1), natural = before.top + before.scrollTop;
      await page.click('#lanes .tree-all[data-id="' + c1 + '"]');
      await page.waitForTimeout(150);
      const outer = await groupOf(page, fan.id), innerOpen = await groupOf(page, c1);
      const stuckIds = await page.evaluate(() => [...document.querySelectorAll("#lanes .tree-row.stuck")].map((x) => x.closest(".treeitem").dataset.id));
      R["nestedInner" + tag] = { outer, innerOpen, stuckIds };
      r.expect(outer && JSON.stringify(outer.ids) === JSON.stringify(fanShort) && outer.all?.text === "All 12" && !outer.stuck, "nested " + tag + ": Fan-out stays a short list, listing Reader 1, and is not sticky: " + JSON.stringify(outer?.ids.map(nname)));
      r.expect(innerOpen && JSON.stringify(innerOpen.ids) === JSON.stringify(nestKids) && innerOpen.stuck && innerOpen.fewer === "Show fewer" && !innerOpen.all, "nested " + tag + ": Reader 1 lists all five helpers with a 'Show fewer': " + JSON.stringify([innerOpen?.ids.map(nname), innerOpen?.stuck, innerOpen?.fewer]));
      r.expect(stuckIds.length === 1 && stuckIds[0] === c1, "nested " + tag + ": only the innermost row is sticky: " + JSON.stringify(stuckIds));
      r.expect(await page.evaluate((id) => { const a = document.activeElement; return a?.classList.contains("tree-fewer") && a.closest(".treeitem").dataset.id === id; }, c1), "nested " + tag + ": focus is on Reader 1's 'Show fewer', not the page");
      const seen = [];
      const reach = (await stickyBox(page, c1)).max;
      for (const d of [30, 90]) {
        if (natural + d > reach) continue;
        await scrollSidebar(page, natural + d);
        const at = await stickyBox(page, c1);
        seen.push(at);
        r.expect(at && at.stuck && Math.abs(at.top) <= 1 && at.fewerVisible, "nested " + tag + ": scrolled to " + at?.scrollTop + " Reader 1's row holds the sidebar's top pixel: " + JSON.stringify(at));
      }
      R["nestedSticky" + tag] = { natural, seen };
      r.expect(seen.length >= 1, "nested " + tag + ": the nested sticky row was checked at a scroll position at least: " + JSON.stringify({ natural, reach }));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-" + tag + "-nested.png") });

      // Collapsing an ancestor drops the open list; "Show fewer" folds it and focuses Reader 1's row.
      if (!dark) {
        await page.evaluate((id) => [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
        await page.waitForTimeout(120);
        const shut = await groupOf(page, fan.id);
        r.expect(shut && shut.expanded === "false" && await page.evaluate(() => document.querySelectorAll("#lanes .tree-row.stuck").length) === 0, "nested " + tag + ": collapsing an ancestor drops the open list: " + JSON.stringify([shut?.expanded]));
        await page.evaluate((id) => [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
        await page.waitForTimeout(120);
        await page.click('#lanes .tree-all[data-id="' + c1 + '"]');
        await page.waitForTimeout(120);
      }
      await page.click("#lanes .tree-row.stuck .tree-fewer");
      await page.waitForTimeout(150);
      const folded = await groupOf(page, c1);
      r.expect(folded && JSON.stringify(folded.ids) === JSON.stringify(nestShort) && folded.all?.text === "All 5" && !folded.stuck, "nested " + tag + ": 'Show fewer' folds Reader 1 back to three rows: " + JSON.stringify(folded?.ids.map(nname)));
      r.expect(await page.evaluate((id) => document.activeElement?.classList.contains("srow") && document.activeElement.dataset.id === id, c1), "nested " + tag + ": focus is on Reader 1's row after folding");

      if (!dark) {
        // A live update while Swarm's whole list is open: it stays open, reorders, takes the new child, and keeps focus on "Show fewer".
        await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 0; });
        await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
        await page.waitForTimeout(150);
        state.edit = finishSwarm;
        await page.waitForFunction((id) => document.querySelectorAll('#lanes .treeitem[data-id="' + id + '"] > .tree-group > .treeitem').length === 13, swarm.id, { timeout: 15000 });
        await page.waitForTimeout(100);
        const after = await groupOf(page, swarm.id);
        R.liveAfter = after;
        r.expect(after && JSON.stringify(after.ids) === JSON.stringify(live), "live: the open list reorders (Worker 10 finished) and takes Worker 13: " + JSON.stringify(after?.ids.map((id) => finishSwarm(nest(structuredClone(D.model))).sessions[id].name)));
        r.expect(after?.stuck === true && after.fewer === "Show fewer" && after.dialogs === 0, "live: the list is still open and sticky after the update: " + JSON.stringify([after?.stuck, after?.fewer]));
        r.expect(await page.evaluate((id) => { const a = document.activeElement; return a?.classList.contains("tree-fewer") && a.closest(".treeitem").dataset.id === id; }, swarm.id), "live: focus stays on 'Show fewer' through the redraw");
        await page.click("#lanes .tree-row.stuck .tree-fewer");
      }
      r.expect(page.errors.length === 0, "nested " + tag + ": page errors " + page.errors.join("; "));
      await page.context().close();
    }

    // Crossing 760 px drops the open list (and a phone's sheet closes when the screen turns wide).
    {
      const page = await served(browser, { extras: true, size: "desktop", dark: false });
      await page.waitForSelector("#lanes .treeitem");
      await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
      await page.waitForTimeout(150);
      r.expect((await groupOf(page, swarm.id))?.stuck === true, "resize: the list is open and sticky at 1280");
      await page.setViewportSize({ width: 390, height: 844 });
      await page.waitForTimeout(300);
      const narrow = await groupOf(page, swarm.id);
      R.resizeNarrow = narrow;
      r.expect(narrow && !narrow.stuck && JSON.stringify(narrow.ids) === JSON.stringify(swarmShort) && narrow.all?.text === "All 12", "resize: at 390 the sticky list is gone and the short list is back: " + JSON.stringify([narrow?.stuck, narrow?.ids.map(name), narrow?.all?.text]));
      await openDrawer(page);
      await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
      await page.waitForSelector("dialog.kids-sheet[open]");
      await page.setViewportSize({ width: 1280, height: 860 });
      await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
      await page.waitForTimeout(300);
      const wide = await groupOf(page, swarm.id);
      R.resizeWide = wide;
      r.expect(wide && !wide.stuck && JSON.stringify(wide.ids) === JSON.stringify(swarmShort) && wide.all?.text === "All 12" && wide.dialogs === 0, "resize: at 1280 the sheet is closed and the short list stands: " + JSON.stringify([wide?.stuck, wide?.dialogs]));
      await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
      await page.waitForTimeout(150);
      r.expect((await groupOf(page, swarm.id))?.stuck === true, "resize: back at 1280 'All 12' opens the list inline again");
      // The rail drops it as well.
      await page.click("#rail-toggle"); await page.waitForTimeout(250);
      await page.click("#rail-toggle"); await page.waitForTimeout(250);
      const railed = await groupOf(page, swarm.id);
      r.expect(railed && !railed.stuck && JSON.stringify(railed.ids) === JSON.stringify(swarmShort), "resize: going to the rail and back drops the open list: " + JSON.stringify([railed?.stuck, railed?.ids.map(name)]));
      r.expect(page.errors.length === 0, "resize: page errors " + page.errors.join("; "));
      await page.context().close();
    }

    // The phone's sheet with a grandchild: Fan-out's sheet names the parent of Reader 1's helpers.
    {
      const { page } = await servedModel(browser, { extras: true, size: "phone", dark: false }, nest);
      await openDrawer(page);
      await page.click('#lanes .tree-all[data-id="' + fan.id + '"]');
      await page.waitForSelector("dialog.kids-sheet[open]");
      await page.waitForTimeout(150);
      const sheet = await sheetOf(page);
      R.phoneNestedSheet = sheet;
      r.expect(sheet && sheet.ids.length === 12 && sheet.count === "12 sessions" && JSON.stringify(sheet.headings) === JSON.stringify(["Running (2)", "Finished (10)"]), "nested phone: Fan-out's sheet lists all 12 descendants: " + JSON.stringify([sheet?.count, sheet?.headings]));
      r.expect(sheet && sheet.unders.filter(Boolean).length === 5 && sheet.unders.filter(Boolean).every((u) => u === "under Reader 1") && sheet.ids.every((id, i) => (sheet.unders[i] !== null) === nestKids.includes(id)), "nested phone: only the five helpers say 'under Reader 1': " + JSON.stringify(sheet?.unders));
      r.expect(sheet && sheet.sideways === 0 && sheet.minRow >= 44, "nested phone: the suffix adds no overflow and no short row: " + JSON.stringify([sheet?.sideways, sheet?.minRow]));
      const c = await contrast(page, "dialog.kids-sheet .kids-row .under");
      r.expect(c.length === 5 && c.every((x) => x.ratio >= 4.5), "nested phone: the suffix meets AA contrast: " + JSON.stringify(c));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light-sheet-nested.png") });
      await page.keyboard.press("Escape");
      await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
      r.expect(await page.evaluate(() => document.body.classList.contains("drawer-open")), "nested phone: Esc leaves the drawer open");
      await page.context().close();
    }
  }

  // ---- The list scrolls, the rest of the sidebar and "All sessions" stay put -------------------------------------------
  // A lane with no spawned sessions and no parent gives a query that leaves one row, so the list is short.
  const spawned = new Set(D.H.filter((h) => h.kind === "spawn").flatMap((h) => [h.from, h.to]));
  const lone = Object.values(D.SESS).find((s) => s.lane && !spawned.has(s.id));
  for (const [size, tagSize] of [["phone", "390"], ["desktop", "1280"]]) {
    for (const dark of [false, true]) {
      const tag = dark ? "dark" : "light", P = size + " " + tag;
      const page = await served(browser, { extras: true, size, dark });
      await page.waitForSelector("#lanes .treeitem", { state: "attached" });
      if (size === "phone") await openDrawer(page);
      const top = await sideGeometry(page);
      r.expect(top.overflowing, P + ": the fixture's list must overflow or this proves nothing: " + JSON.stringify(top));
      r.expect(top.sidebarTop === 0 && top.listTop === 0 && top.footShown && top.footTop >= 0 && top.footBottom <= top.vh, P + ": at rest 'All sessions' is inside the screen: " + JSON.stringify(top));
      r.expect(top.footTop >= top.listBottom - 0.5, P + ": 'All sessions' sits below the list, not over it: " + JSON.stringify(top));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-top.png") });
      for (const where of ["mid", "end"]) {
        await scrollList(page, where);
        const g = await sideGeometry(page);
        R["scroll" + P + where] = g;
        r.expect(g.listTop > 0, P + ": the list scrolled to " + where + ": " + JSON.stringify(g));
        r.expect(g.sidebarTop === 0, P + ": the sidebar itself did not scroll (" + where + "): scrollTop " + g.sidebarTop);
        r.expect(JSON.stringify([g.brand, g.search, g.nav]) === JSON.stringify([top.brand, top.search, top.nav]), P + ": header, search and nav kept their boxes (" + where + "): " + JSON.stringify([g.brand, g.search, g.nav]) + " vs " + JSON.stringify([top.brand, top.search, top.nav]));
        r.expect(g.footShown && g.footTop >= 0 && g.footBottom <= g.vh && g.footTop === top.footTop, P + ": 'All sessions' stays fully on screen and does not move (" + where + "): " + JSON.stringify(g));
        if (where === "mid") await page.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-mid.png") });
      }
      r.expect((await sideGeometry(page)).atEnd, P + ": the list reaches its end");
      // Keyboard focus scrolls the row into the list, never under the footer.
      await scrollList(page, "top");
      await page.evaluate(() => [...document.querySelectorAll("#lanes .srow")].filter((e) => e.getClientRects().length).at(-1).focus());
      const focused = await page.evaluate(() => { const f = document.activeElement.getBoundingClientRect(), l = document.querySelector("#side-list").getBoundingClientRect(), a = document.querySelector("#all-sessions").getBoundingClientRect(); return { top: Math.round(f.top), bottom: Math.round(f.bottom), listTop: Math.round(l.top), listBottom: Math.round(l.bottom), footTop: Math.round(a.top), sidebar: document.querySelector("#sidebar").scrollTop }; });
      R["focus" + P] = focused;
      r.expect(focused.bottom <= focused.listBottom && focused.bottom <= focused.footTop && focused.top >= focused.listTop && focused.sidebar === 0, P + ": the last row, focused, sits inside the list and above the footer: " + JSON.stringify(focused));
      r.expect(await overflow(page) === 0, P + ": no sideways overflow");
      // A short list: the footer follows the last row.
      if (lone) {
        await page.fill("#q", lone.name); await page.waitForTimeout(150);
        const g = await sideGeometry(page);
        R["short" + P] = g;
        r.expect(!g.overflowing && g.lastRowBottom !== null, P + ": a query for '" + lone.name + "' leaves a short list: " + JSON.stringify(g));
        r.expect(g.footTop - g.lastRowBottom >= 0 && g.footTop - g.lastRowBottom <= 16, P + ": a short list leaves 'All sessions' right after its last row: gap " + (g.footTop - g.lastRowBottom));
        if (!dark) await page.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-short.png") });
      } else r.expect(false, "the extras fixture has no lone lane for the short-list check");
      // The rail: still toggles, no footer, no sidebar scroll.
      if (size === "desktop" && !dark) {
        await page.fill("#q", ""); await page.waitForTimeout(100);
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        const g = await sideGeometry(page);
        R.railScroll = g;
        r.expect(await page.evaluate(() => document.querySelector(".app").classList.contains("rail")), "rail: the toggle still collapses the sidebar");
        r.expect(!g.footShown && g.sidebarTop === 0, "rail: no 'All sessions' footer and the sidebar does not scroll: " + JSON.stringify(g));
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        r.expect(!(await page.evaluate(() => document.querySelector(".app").classList.contains("rail"))) && (await sideGeometry(page)).footShown, "rail: the toggle expands it again and the footer returns");
      }
      r.expect(page.errors.length === 0, P + ": page errors " + page.errors.join("; "));
      await page.context().close();
    }
  }

  // ---- The open session's row is the only one marked ----------------------------------------------------------------
  {
    // The newest child the short list leaves out when nothing is open.
    const child = fanKids.find((c) => !fanShort.includes(c.id));
    const page = await served(browser, { extras: true, size: "desktop", dark: false });
    await page.waitForSelector("#lanes .treeitem");
    const marks = () => page.evaluate(({ parent, child }) => {
      const cur = [...document.querySelectorAll("#lanes [aria-current='page']")].map((x) => x.dataset.id);
      const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent);
      const pick = (id) => item?.querySelector('.srow[data-id="' + CSS.escape(id) + '"]') ?? (id === parent ? item?.querySelector(":scope > .tree-row .srow") : null);
      return { current: cur, expanded: item?.getAttribute("aria-expanded"), childShown: !!pick(child), rows: item?.querySelectorAll(":scope > .tree-group > .treeitem").length ?? 0, onPath: !!item?.querySelector(":scope > .tree-row .srow.on-path"), ring: item?.querySelector(":scope > .tree-row .srow.on-path")?.getAttribute("aria-current") ?? null, ringColor: item?.querySelector(":scope > .tree-row .srow.on-path") ? getComputedStyle(item.querySelector(":scope > .tree-row .srow.on-path")).boxShadow : null, all: item?.querySelector(":scope > .tree-group > .tree-all")?.textContent ?? null,
        parentBg: getComputedStyle(item.querySelector(":scope > .tree-row .srow")).backgroundColor, childBg: pick(child) ? getComputedStyle(pick(child)).backgroundColor : null };
    }, { parent: fan.id, child: child.id });
    const pref = () => page.evaluate((id) => JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[id] ?? null, fan.id);
    // The parent is collapsed by a saved choice.
    await page.evaluate((id) => localStorage.setItem("semon.tree", JSON.stringify({ [id]: { open: false, at: 1 } })), fan.id);
    await page.reload({ waitUntil: "load" });
    await page.waitForSelector("#lanes .treeitem");
    await goto(page, { v: "sessions" }, D);
    const before = await marks();
    r.expect(before.expanded === "false" && before.current.length === 0, "open: a saved collapse holds while no child is open: " + JSON.stringify(before));

    await goto(page, { v: "session", id: child.id }, D);
    const open = await marks();
    R.currentChild = { child: child.id, open, pref: await pref() };
    r.expect(open.current.length === 1 && open.current[0] === child.id, "open: only the open child's row is current, not its parent: " + JSON.stringify(open.current));
    r.expect(open.expanded === "true" && open.childShown, "open: the collapsed parent opens to show the open child: " + JSON.stringify(open));
    r.expect(open.rows === 3 && open.all === "All 7", "open: the open child takes one of the three rows, and 'All 7' stays: " + JSON.stringify([open.rows, open.all]));
    const saved = await pref();
    r.expect(saved?.open === false && saved.at === 1 && !("more" in saved), "open: the parent's saved choice is unchanged while its child is open: " + JSON.stringify(saved));
    r.expect(open.childBg !== "rgba(0, 0, 0, 0)" && open.parentBg === "rgba(0, 0, 0, 0)", "open: only the open child is highlighted: parent " + open.parentBg + ", child " + open.childBg);
    await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-light-child-open.png") });

    // The parent opens for the child's page alone: leaving it restores the saved choice.
    await goto(page, { v: "sessions" }, D);
    const away = await marks();
    r.expect(away.expanded === "false" && away.current.length === 0, "open: leaving the child restores the parent's saved collapse: " + JSON.stringify(away));
    r.expect(JSON.stringify(await pref()) === JSON.stringify(saved), "open: the saved choice is still unchanged after navigating away: " + JSON.stringify(await pref()));

    // A collapse made while the child is open sticks through redraws (a typed search and clearing it redraw the tree, as a live update does).
    await goto(page, { v: "session", id: child.id }, D);
    r.expect((await marks()).expanded === "true", "open: the parent opens again for the next visit to the child");
    await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
    await page.fill("#q", "Reader"); await page.waitForTimeout(120);
    await page.fill("#q", ""); await page.waitForTimeout(120);
    const stuck = await marks();
    R.collapseSticks = stuck;
    r.expect(stuck.expanded === "false" && stuck.current.length === 1 && stuck.current[0] === child.id, "open: a collapse made while the child is open survives redraws: " + JSON.stringify(stuck));

    // Rail: the children are hidden, so the top-level ancestor keeps a quiet ring, not the highlight.
    await goto(page, { v: "session", id: child.id }, D);
    await page.click("#rail-toggle"); await page.waitForTimeout(300);
    const rail = await marks();
    R.rail = rail;
    r.expect(rail.current.length === 0 && rail.onPath && rail.parentBg === "rgba(0, 0, 0, 0)", "rail: the ancestor has a ring and no highlight, and nothing is the current page: " + JSON.stringify(rail));
    r.expect(rail.ring === "true", "rail: the ringed ancestor carries aria-current=\"true\" for screen readers: " + rail.ring);
    r.expect(rail.ringColor?.startsWith("rgb(93, 101, 97)"), "rail: the ring is --muted, not --faint: " + rail.ringColor);
    r.expect(page.errors.length === 0, "open: page errors " + page.errors.join("; "));
    await page.context().close();
  }
  // ---- The nav order: Home, Sessions, Analytics, Machines, in the sidebar, the rail and the drawer ------------------------
  {
    const order = (page) => page.evaluate(() => [...document.querySelectorAll("#nav .nav-item")].map((b) => ({ go: b.dataset.go, label: b.querySelector("span:not(.cnt)")?.textContent, shown: b.getClientRects().length > 0, top: Math.round(b.getBoundingClientRect().top) })));
    const want = ["home", "sessions", "analytics", "machines"], labels = ["Home", "Sessions", "Analytics", "Machines"];
    const ok = (o) => JSON.stringify(o.map((b) => b.go)) === JSON.stringify(want) && JSON.stringify(o.map((b) => b.label)) === JSON.stringify(labels) && o.every((b) => b.shown) && o.every((b, i) => i === 0 || b.top > o[i - 1].top);
    const desktop = await served(browser, { extras: true, size: "desktop", dark: false });
    await desktop.waitForSelector("#nav .nav-item");
    const expanded = await order(desktop);
    R.navExpanded = expanded;
    r.expect(ok(expanded), "nav: the expanded sidebar lists Home, Sessions, Analytics, Machines top to bottom: " + JSON.stringify(expanded));
    await desktop.click("#rail-toggle"); await desktop.waitForTimeout(300);
    const rail = await order(desktop);
    R.navRail = rail;
    r.expect(rail.map((b) => b.go).join() === want.join() && rail.every((b) => b.shown) && rail.every((b, i) => i === 0 || b.top > rail[i - 1].top), "nav: the rail's icons run Home, Sessions, Analytics, Machines top to bottom: " + JSON.stringify(rail));
    await desktop.screenshot({ path: path.join(ENV.out, "sidebar-nav-1280-light-rail.png") });
    r.expect(desktop.errors.length === 0, "nav desktop: page errors " + desktop.errors.join("; "));
    await desktop.context().close();
    for (const dark of [false, true]) {
      const tag = dark ? "dark" : "light", phone = await served(browser, { extras: true, size: "phone", dark });
      await openDrawer(phone);
      const drawer = await order(phone);
      R["navDrawer" + tag] = drawer;
      r.expect(ok(drawer), "nav: the phone drawer (" + tag + ") lists Home, Sessions, Analytics, Machines top to bottom: " + JSON.stringify(drawer));
      await phone.screenshot({ path: path.join(ENV.out, "sidebar-nav-390-" + tag + ".png") });
      await phone.context().close();
    }
    for (const dark of [false, true]) {
      const page = await served(browser, { extras: true, size: "desktop", dark });
      await page.waitForSelector("#nav .nav-item");
      r.expect(ok(await order(page)), "nav: the 1280 sidebar (" + (dark ? "dark" : "light") + ") keeps the order");
      await page.screenshot({ path: path.join(ENV.out, "sidebar-nav-1280-" + (dark ? "dark" : "light") + ".png") });
      await page.context().close();
    }
  }
  return r.done();
}
