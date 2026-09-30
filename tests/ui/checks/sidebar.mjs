// The sidebar tree, on the extras fixture, phone and desktop. It holds two live parents: `Fan-out` (seven subagents, only the
// oldest, Reader 1, still running) and `Swarm` (twelve: Workers 1 to 10 running, 11 and 12 finished and newer).
//   - an open parent lists its waiting children, then its running ones (at most 8), then the newest finished ones until three rows
//     are listed, and ends with one "All N" row when anything is hidden, N being the number of sessions under the parent. Fan-out lists Reader 1
//     (old, but running) ahead of the newer finished Readers 7 and 6; Swarm lists Workers 10 to 3 and no finished worker.
//   - there is no "Show N more" or "Show fewer" list control, and an old saved `more` is ignored and pruned on the next save.
//   - a parent with no waiting or running session below it, and not holding the open session, is collapsed by default.
//   - the keyboard reaches "All N" in tree order; on a phone the row is at least 44 px tall and its text has AA contrast.
//   - on a phone "All N" opens a bottom sheet with a search field and Waiting / Running / Finished sections holding exactly the
//     descendants, newest first; a tap on a row opens that session and closes the sheet; Esc and a tap on the backdrop close it
//     and return focus to "All N"; rows are at least 44 px, there is one scroll surface, no sideways overflow, and AA contrast.
//   - on a wide screen "All N" opens no dialog: it lists the whole parent inline, in the same order; the parent's row sticks to the
//     top of the sidebar while the list scrolls, with "Show fewer", which folds the list, scrolls the parent back into view and
//     focuses it; the open list is not saved (a reload starts short); navigating to a row under the sticky row scrolls it clear.
//   - opening a nested parent's "All N" inside an open parent leaves the ancestors listed and open and sticks only the innermost
//     row; on a wide screen "All N" reveals exactly its number (every descendant, under its own parent, open); a live update keeps
//     the whole list open, its rows in place and focus on "Show fewer" (the pill then reorders it); crossing 760 px, the rail, collapsing the parent or an ancestor, and a list with
//     nothing left to fold all drop the open list; Esc in a phone's sheet leaves the drawer open; the sheet says which parent a
//     grandchild is under.
//   - no parent row shows a count pill, open or collapsed (the chevron says there are children), and a collapsed row says it is collapsed (aria-expanded); the row's aria-label still breaks the total down as text (runs, needs you, working, failed). A patched model puts a waiting and a failed run, and a grandchild, under Fan-out.
//   - the toggle sits over the right end of its row's meta line, and only rows with children have one: no row has a left gutter.
//   - the toggle's box is at least 44x44 at 390 px and at least 28x36 at 1280 px.
//   - the header puts the logo first and the collapse toggle at the right end of the row (on a phone, the drawer's close button
//     there instead), both at least 44x44; in the rail the toggle shows with the logo mark above it, and keyboard focus reaches
//     the toggle from the first nav row and returns in order.
//   - only the open session's row is current: its parent is not highlighted; a collapsed parent opens for the child's page
//     without saving that, and its saved choice is unchanged after navigating away; a collapse made while the child is open
//     stays collapsed through redraws; the open child is always listed; in the rail the ancestor keeps a ring, no highlight, and
//     aria-current="true".
//   - the sidebar as a whole never scrolls: the header and nav keep their boxes (with the "Recent" label) while the session list scrolls on its own,
//     and the list stays inside the screen at 390 and 1280. There is no "All sessions" link under it (the nav's Sessions row
//     goes there); a row focused by keyboard scrolls into the list; the rail still toggles. The "Recent" label stays above the scroller, and a hairline shows at the list's top and foot
//     only while rows lie beyond that edge (read from a screenshot strip: none at rest above the rows, both when scrolled to the
//     middle, none below at the end, none on a short list).
//   - the nav runs Home, Sessions, Analytics, Machines top to bottom in the expanded sidebar, the rail and the phone drawer.
//   - an expanded top-level parent's row pins at the top of the list (position: sticky, opaque, in the sidebar's colour) while its
//     children scroll, in the phone drawer and the 1280x800 sidebar, light and dark: its top equals the list's top, it is the topmost
//     element there, its children are under it and its chevron takes a tap; past the group it goes with it and the next parent is
//     pinned; a focused child scrolls clear of it; a collapsed parent and a nested one do not stick. The model is patched so two
//     parents overflow the list (out/sidebar-sticky-*.png).
//   - screenshots of the sidebar (and of the sheet, and of the sticky list) at 390 and 1280, light and dark, go to out/sidebar-*.png.
import path from "node:path";
import { PNG } from "pngjs";
import { ENV, served, goto, data, reporter, overflow, settled } from "../lib.mjs";

const openDrawer = async (page) => { await page.click("#lead-btn"); await page.waitForTimeout(300); };

// A root's direct children, its "All N" row, how many count pills its row has (none), and what is saved for it.
const groupOf = (page, id) => page.evaluate((id) => {
  const item = [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id);
  if (!item) return null;
  const all = item.querySelector(":scope > .tree-group > .tree-all"), pills = item.querySelectorAll(":scope > .tree-row .tree-summary").length;
  const stored = JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[id] ?? null;
  return {
    ids: [...item.querySelectorAll(":scope > .tree-group > .treeitem")].map((x) => x.dataset.id),
    allIds: [...item.querySelectorAll(".tree-group .treeitem")].map((x) => x.dataset.id),
    label: item.querySelector(":scope > .tree-row .srow")?.getAttribute("aria-label") ?? null,
    all: all ? { text: all.textContent, role: all.getAttribute("role"), label: all.getAttribute("aria-label"), tag: all.tagName, tabIndex: all.tabIndex, height: Math.round(all.getBoundingClientRect().height * 10) / 10 } : null,
    oldButtons: document.querySelectorAll("#lanes .tree-more").length, dialogs: document.querySelectorAll("dialog").length,
    pills, expanded: item.getAttribute("aria-expanded"), stored,
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

// Every top-level parent with children, collapsed in turn (and put back as it was): whether a count pill shows in its row, open
// or collapsed, the row's accessible label, and its attention dot (`.kid-flag`: its state class, colour, and whether it is decorative
// and bare) with what the two tokens resolve to in the page's theme.
const collapsedRows = (page) => page.evaluate(() => {
  const token = (name) => { const p = document.createElement("i"); p.style.background = "var(" + name + ")"; document.body.append(p); const c = getComputedStyle(p).backgroundColor; p.remove(); return c; };
  const want = { wait: token("--wait-dot"), err: token("--err") };
  const pillsIn = (item) => item.querySelectorAll(":scope > .tree-row .tree-summary").length;
  const flagIn = (item) => {
    const fs = [...item.querySelectorAll(":scope > .tree-row .kid-flag")], f = fs[0];
    if (!f) return { count: 0 };
    const b = f.getBoundingClientRect(), ag = item.querySelector(":scope > .tree-row .ag").getBoundingClientRect();
    return { count: fs.length, kind: f.classList.contains("wait") ? "wait" : f.classList.contains("err") ? "err" : null, bg: getComputedStyle(f).backgroundColor, hidden: f.getAttribute("aria-hidden") === "true", bare: f.textContent === "", shown: b.width > 0 && b.height > 0, beforeTime: b.right <= ag.left + 0.5 };
  };
  return [...document.querySelectorAll("#lanes > .treeitem")].filter((item) => item.querySelector(":scope > .tree-row .tree-toggle")).map((item) => {
    const toggle = item.querySelector(":scope > .tree-row .tree-toggle"), wasOpen = item.getAttribute("aria-expanded") === "true", open = wasOpen ? pillsIn(item) : null, flagOpen = wasOpen ? flagIn(item) : null;
    if (wasOpen) toggle.click();
    const out = { id: item.dataset.id, expanded: item.getAttribute("aria-expanded"), pills: pillsIn(item), pillsOpen: open, label: item.querySelector(":scope > .tree-row .srow").getAttribute("aria-label"), flag: flagIn(item), flagOpen, want };
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

// No parent row shows a count pill, open or collapsed (the chevron already says there are children); the row's accessible label
// still carries the count and the breakdown as text (a run that needs you, working, failed), and a collapsed row says it is collapsed.
// A small dot, open or collapsed, shows exactly when a session under the parent needs you (amber, --wait-dot) or failed (red, --err),
// amber first when both; it is decorative (aria-hidden), bare (no number) and before the time, and there is none otherwise.
function assertNoPills(r, where, rows, SESS, H, mustShow) {
  r.expect(rows.length > 0, where + ": no parent row was found");
  for (const p of rows) {
    const t = tally(SESS, H, p.id), parts = breakdown(t), name = SESS[p.id]?.name + " ";
    const kind = t.wait > 0 ? "wait" : t.err > 0 ? "err" : null;
    for (const [state, f] of [["collapsed", p.flag], ...(p.flagOpen ? [["open", p.flagOpen]] : [])]) {
      if (kind) r.expect(f.count === 1 && f.kind === kind && f.bg === p.want[kind] && f.hidden && f.bare && f.shown && f.beforeTime, where + " " + name + " (" + state + "): one bare, decorative " + kind + " dot in " + p.want[kind] + " before the time, as " + t.wait + " wait and " + t.err + " failed sit under it: " + JSON.stringify(f));
      else r.expect(f.count === 0, where + " " + name + " (" + state + "): no attention dot with nothing waiting or failed under it: " + JSON.stringify(f));
    }
    r.expect(p.pills === 0 && !p.pillsOpen, where + " " + name + ": a parent row shows no count pill, collapsed (" + p.pills + ") or open (" + p.pillsOpen + ")");
    r.expect(p.expanded === "false", where + " " + name + ": the collapsed row says it is collapsed (aria-expanded): " + p.expanded);
    r.expect(p.label?.endsWith(", " + parts.join(", ")), where + " " + name + ": the row's aria-label carries the count and breakdown as text: " + JSON.stringify(p.label));
  }
  for (const name of mustShow) r.expect(rows.some((p) => SESS[p.id]?.name === name), where + ": " + name + " has no parent row");
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
const fanReviewIds = ["guardian-review-canonical", "guardian-review-nested"];
const approvalRootId = "guardian-review-root";
const approvalReviewIds = [...fanReviewIds, approvalRootId];
const approvalHelperId = "approval-review-name-only";
const approvalBridgeId = "review-follow-up-worker";
const approvalRootWorkerId = "root-review-worker";
const approvalFixtureIds = new Set([...approvalReviewIds, approvalHelperId, approvalBridgeId, approvalRootWorkerId]);
const patchApprovalReviews = (m, parentId, sourceId) => {
  const source = m.sessions[sourceId], newest = Math.max(0, ...Object.values(m.sessions).map((s) => Number(s.last) || 0), ...(m.handoffs ?? []).map((h) => Number(h.at) || 0));
  const add = (id, name, kind, parent, lane, offset) => {
    m.sessions[id] = { ...structuredClone(source), id, name, kind, harness: "codex", lane, stub: false, parent, state: "done", last: newest + offset };
  };
  add(fanReviewIds[0], "Approval review", "Approval review", parentId, false, 2);
  add(fanReviewIds[1], "Approval review", "Approval review", parentId, false, 3);
  add(approvalHelperId, "Approval review helper", "Codex run", parentId, false, 1);
  add(approvalBridgeId, "Follow-up worker", "Codex run", fanReviewIds[0], false, 4);
  add(approvalRootId, "Approval review", "Approval review", null, true, 6);
  add(approvalRootWorkerId, "Root review worker", "Codex run", approvalRootId, false, 5);
  return m;
};
const servedApprovalPatched = async (browser, opts, parentId, sourceId, baselineData) => {
  const page = await served(browser, opts);
  await goto(page, { v: "sessions" }, baselineData);
  const baseline = await page.evaluate(() => ({
    count: Number(document.querySelector(".ph .sub span:first-child b")?.textContent),
    ids: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id),
    reviewControl: !!document.querySelector('#page .groupby[aria-label="Session visibility"]'),
  }));
  page.approvalBaseSessionCount = baseline.count;
  page.approvalBaseHasReviewControl = baseline.reviewControl;
  await page.route((u) => u.pathname === "/api/model" && !u.searchParams.has("since"), async (route) => {
    const res = await route.fetch(), m = patchApprovalReviews(await res.json(), parentId, sourceId);
    await route.fulfill({ status: res.status(), headers: { "content-type": "application/json" }, body: JSON.stringify(m) });
  });
  await page.route((u) => u.pathname === "/api/tx" && approvalFixtureIds.has(u.searchParams.get("sid")), async (route) => {
    const url = new URL(route.request().url()); url.searchParams.set("sid", sourceId);
    const res = await route.fetch({ url: url.toString() });
    await route.fulfill({ status: res.status(), headers: { "content-type": "application/json" }, body: await res.text() });
  });
  // These synthetic fixture IDs exist only in the browser-side model override, so serve the shell at their deep-link paths.
  await page.route((u) => u.pathname.startsWith("/s/codex/") && approvalFixtureIds.has(decodeURIComponent(u.pathname.split("/").at(-1))), async (route) => {
    const url = new URL(route.request().url()); url.pathname = "/";
    const res = await route.fetch({ url: url.toString() });
    await route.fulfill({ status: res.status(), headers: { "content-type": "text/html; charset=utf-8" }, body: await res.text() });
  });
  await page.reload({ waitUntil: "load" }); await settled(page);
  return page;
};
// The sidebar's boxes and the list's scroll state, in one read.
const sideGeometry = (page) => page.evaluate(() => {
  const r4 = (e) => { if (!e) return null; const b = e.getBoundingClientRect(); return [b.left, b.top, b.width, b.height].map((n) => Math.round(n * 10) / 10); };
  const sb = document.querySelector("#sidebar"), list = document.querySelector("#side-list");
  const rows = [...document.querySelectorAll("#lanes .srow")].filter((e) => e.getClientRects().length);
  const last = rows.at(-1)?.getBoundingClientRect(), l = list.getBoundingClientRect();
  return {
    brand: r4(document.querySelector(".brandrow")), nav: r4(document.querySelector("#nav")),
    recent: r4(document.querySelector(".side-h")), recentInList: !!document.querySelector("#side-list .side-h"),
    sidebarTop: sb.scrollTop, listTop: Math.round(list.scrollTop), overflowing: list.scrollHeight > list.clientHeight + 1, atEnd: list.scrollTop + list.clientHeight >= list.scrollHeight - 1,
    allLink: !!document.querySelector("#all-sessions, #lanes-all, .side-all"),
    listBottom: Math.round(l.bottom * 10) / 10, lastRowBottom: last ? Math.round(last.bottom * 10) / 10 : null, rows: rows.length, vh: innerHeight,
  };
});
// Whether a hairline shows at the list's top edge and at its foot: a 1px-wide strip at the list's left edge (clear of every row) is
// screenshotted, and any pixel that differs from the sidebar's own colour counts as a line.
const edges = async (page) => {
  const box = await page.evaluate(() => {
    const l = document.querySelector("#side-list").getBoundingClientRect(), probe = document.createElement("i");
    probe.style.background = "var(--side)"; document.body.append(probe); const side = getComputedStyle(probe).backgroundColor; probe.remove();
    return { x: l.left + 2, top: l.top, bottom: l.bottom, side };
  });
  const rgb = box.side.match(/\d+/g).slice(0, 3).map(Number);
  const strip = async (y) => {
    const png = PNG.sync.read(await page.screenshot({ clip: { x: Math.floor(box.x), y: Math.floor(y), width: 1, height: 6 } }));
    for (let i = 0; i < png.data.length; i += 4) if ([0, 1, 2].some((k) => Math.abs(png.data[i + k] - rgb[k]) > 6)) return true;
    return false;
  };
  return { top: await strip(box.top - 2), bottom: await strip(box.bottom - 4) };
};
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

// Sticky rows stick to the top of the sidebar's one scroller, #side-list (the header and nav sit above it).
const stickyBox = (page, id) => page.evaluate((id) => {
  const sb = document.querySelector("#side-list"), item = [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id), row = item?.querySelector(":scope > .tree-row");
  if (!row) return null;
  const s = sb.getBoundingClientRect(), r = row.getBoundingClientRect(), f = row.querySelector(".tree-fewer")?.getBoundingClientRect();
  return { top: Math.round((r.top - s.top) * 10) / 10, bottom: Math.round((r.bottom - s.top) * 10) / 10, scrollTop: Math.round(sb.scrollTop), max: Math.round(sb.scrollHeight - sb.clientHeight), stuck: row.classList.contains("stuck"), fewerVisible: !!f && f.top >= s.top - 0.5 && f.bottom <= s.bottom + 0.5, viewport: Math.round(s.height) };
}, id);
// A page whose model is patched on the way in: `patch(model)` runs on every full model the page fetches, and setting `state.edit` to a
// function makes the next poll return the model changed by it (a live update), once.
const servedModel = async (browser, opts, patch, baselineData = null) => {
  const page = await served(browser, opts), state = { edit: null, version: null, n: 0 };
  let baseline = null;
  if (baselineData) {
    await goto(page, { v: "sessions" }, baselineData);
    baseline = await page.evaluate(() => ({
      count: Number(document.querySelector(".ph .sub span:first-child b")?.textContent),
      ids: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id),
    }));
  }
  await page.route((u) => u.pathname === "/api/model", async (route) => {
    const url = new URL(route.request().url()), since = url.searchParams.get("since");
    if (since !== null && !state.edit) return state.version && since === state.version ? route.fulfill({ status: 304 }) : route.continue();
    url.searchParams.delete("since");
    const res = await route.fetch({ url: url.toString() }), m = patch(await res.json());
    if (since !== null) { state.edit(m); m.version = state.version = "live-" + (++state.n); state.edit = null; }
    await route.fulfill({ status: 200, headers: { "content-type": "application/json" }, body: JSON.stringify(m) });
  });
  await page.reload({ waitUntil: "load" }); await settled(page);
  return { page, state, baseline };
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

  // Approval reviews stay in the model and accounting, while the session list and navigation hide them by default.
  const baseOtherRuns = below(fan.id).filter((s) => s.kind !== "Subagent" && s.kind !== "Codex run").length;
  const baseCodexRuns = below(fan.id).filter((s) => s.kind === "Codex run").length;
  for (const [size, sizeTag] of [["phone", "390"], ["desktop", "1280"]]) for (const dark of [false, true]) {
    const theme = dark ? "dark" : "light", page = await servedApprovalPatched(browser, { extras: true, size, dark }, fan.id, fanKids[0].id, D);
    r.expect(!page.approvalBaseHasReviewControl, size + " " + theme + ": a dataset without approval reviews keeps the existing Sessions controls");
    if (size === "phone") await openDrawer(page);
    const defaultGroup = await groupOf(page, fan.id);
    r.expect(defaultGroup && defaultGroup.ids.includes(approvalBridgeId) && !fanReviewIds.some((id) => defaultGroup.ids.includes(id)) && defaultGroup.all?.text === "All 9" && defaultGroup.label?.includes("9 runs"), size + " " + theme + ": the default sidebar hides review rows, bridges their worker, and updates count and state summary: " + JSON.stringify(defaultGroup));
    const hiddenRootPlacement = await page.evaluate((ids) => ({
      reviewVisible: !!document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(ids.review) + '"]'),
      workerPromoted: !!document.querySelector('#lanes > .treeitem[data-id="' + CSS.escape(ids.worker) + '"]'),
    }), { review: approvalRootId, worker: approvalRootWorkerId });
    r.expect(!hiddenRootPlacement.reviewVisible && hiddenRootPlacement.workerPromoted, size + " " + theme + ": a normal descendant of a hidden root review is promoted to a navigation root: " + JSON.stringify(hiddenRootPlacement));
    if (size === "phone" && defaultGroup?.all) {
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-all").click();
      await page.waitForFunction(() => document.querySelector("dialog.kids-sheet")?.open === true);
      const hiddenSheet = await page.evaluate(() => ({ count: document.querySelector("dialog.kids-sheet .vm")?.textContent, ids: [...document.querySelectorAll("dialog.kids-sheet .kids-row")].map((x) => x.dataset.id) }));
      r.expect(hiddenSheet.count === "9 sessions" && hiddenSheet.ids.includes(approvalBridgeId) && !fanReviewIds.some((id) => hiddenSheet.ids.includes(id)), size + " " + theme + ": All N bridges the normal worker and omits hidden review rows: " + JSON.stringify(hiddenSheet));
      await page.locator("dialog.kids-sheet .kids-search input").fill("Approval review");
      const hiddenSearch = await page.locator("dialog.kids-sheet .kids-row").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      r.expect(hiddenSearch.includes(approvalHelperId) && !approvalReviewIds.some((id) => hiddenSearch.includes(id)), size + " " + theme + ": sidebar search returns the misleading ordinary child without exposing hidden reviews: " + JSON.stringify(hiddenSearch));
      await page.locator("dialog.kids-sheet .kids-search input").fill("Follow-up worker");
      const bridgedSearch = await page.locator("dialog.kids-sheet .kids-row").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      r.expect(JSON.stringify(bridgedSearch) === JSON.stringify([approvalBridgeId]), size + " " + theme + ": sidebar search finds the normal descendant through its hidden review parent: " + JSON.stringify(bridgedSearch));
      await page.locator("dialog.kids-sheet .kids-search input").fill("");
      await page.locator("dialog.kids-sheet .vclose").click();
      await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
    } else if (size === "desktop" && defaultGroup?.all) {
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-all").click();
      await page.waitForFunction((id) => !!document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] .tree-fewer'), fan.id);
      const fullDefaultGroup = await groupOf(page, fan.id);
      r.expect(fullDefaultGroup.ids.includes(approvalHelperId) && fullDefaultGroup.ids.includes(approvalBridgeId) && !fanReviewIds.some((id) => fullDefaultGroup.ids.includes(id)), size + " " + theme + ": the expanded default sidebar bridges the ordinary child and hides reviews");
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-fewer").click();
    }

    await goto(page, { v: "sessions" }, D);
    const defaultList = await page.evaluate(() => ({
      count: Number(document.querySelector(".ph .sub span:first-child b")?.textContent),
      ids: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id),
    }));
    const expectedDefaultCount = page.approvalBaseSessionCount + 3;
    r.expect(defaultList.count === expectedDefaultCount && defaultList.ids.includes(approvalHelperId) && defaultList.ids.includes(approvalBridgeId) && defaultList.ids.includes(approvalRootWorkerId) && !approvalReviewIds.some((id) => defaultList.ids.includes(id)), size + " " + theme + ": session totals hide exact review kinds but retain their ordinary descendants: " + JSON.stringify([defaultList.count, expectedDefaultCount, defaultList.ids.filter((id) => [approvalHelperId, approvalBridgeId, approvalRootWorkerId].includes(id)), approvalReviewIds.filter((id) => defaultList.ids.includes(id))]));
    const fanRow = page.locator("#page .nrow[data-id=\"" + fan.id + "\"]");
    const otherCount = (text) => Number(text.match(/(\d+) other runs?/)?.[1] ?? 0);
    const codexCount = (text) => Number(text.match(/(\d+) Codex runs?/)?.[1] ?? 0);
    r.expect(otherCount(await fanRow.locator(".kids").innerText().catch(() => "")) === baseOtherRuns, size + " " + theme + ": default child counts exclude the two hidden review rows");
    r.expect(codexCount(await fanRow.locator(".kids").innerText().catch(() => "")) === baseCodexRuns + 2, size + " " + theme + ": child counts include the ordinary worker beneath the hidden review");
    await page.locator("#sq").fill("Approval review");
    const searchIds = await page.locator("#page .nrow").evaluateAll((rows) => rows.map((x) => x.dataset.id));
    r.expect(searchIds.includes(approvalHelperId) && !approvalReviewIds.some((id) => searchIds.includes(id)), size + " " + theme + ": session search returns the misleading ordinary child without exposing hidden reviews: " + JSON.stringify(searchIds));
    await page.locator("#sq").fill("");

    await page.locator('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]').click();
    await page.waitForFunction(() => document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]')?.getAttribute("aria-pressed") === "true");
    const shownList = await page.evaluate(() => ({
      count: Number(document.querySelector(".ph .sub span:first-child b")?.textContent),
      ids: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id),
      reviewLabels: [...document.querySelectorAll("#page .nrow")].filter((x) => x.dataset.id.startsWith("guardian-review-")).map((x) => ({ name: x.querySelector(".nm")?.textContent, label: x.querySelector(".for")?.textContent })),
    }));
    r.expect(shownList.count === expectedDefaultCount + 3 && approvalReviewIds.every((id) => shownList.ids.includes(id)) && [approvalHelperId, approvalBridgeId, approvalRootWorkerId].every((id) => shownList.ids.includes(id)) && shownList.reviewLabels.length === approvalReviewIds.length && shownList.reviewLabels.every((x) => x.name === "Approval review" && x.label.includes("Approval review")), size + " " + theme + ": the toggle reveals all labeled reviews and retains their ordinary descendants: " + JSON.stringify([shownList.count, shownList.reviewLabels]));
    r.expect(otherCount(await fanRow.locator(".kids").innerText().catch(() => "")) === baseOtherRuns + fanReviewIds.length, size + " " + theme + ": child counts include the Fan-out reviews only while enabled");
    r.expect(codexCount(await fanRow.locator(".kids").innerText().catch(() => "")) === baseCodexRuns + 2, size + " " + theme + ": child counts retain both ordinary Fan-out descendants");
    if (size === "phone") await openDrawer(page);
    const shownGroup = await groupOf(page, fan.id);
    r.expect(shownGroup?.allIds.includes(approvalBridgeId) && shownGroup?.all?.text === "All 11" && shownGroup.label?.includes("11 runs"), size + " " + theme + ": the enabled sidebar keeps the ordinary descendant and includes reviews in count and state summary: " + JSON.stringify(shownGroup));
    const shownRootPlacement = await page.evaluate((ids) => {
      const review = document.querySelector('#lanes > .treeitem[data-id="' + CSS.escape(ids.review) + '"]');
      return { reviewRoot: !!review, workerNested: !!review?.querySelector('.treeitem[data-id="' + CSS.escape(ids.worker) + '"]'), workerPromoted: !!document.querySelector('#lanes > .treeitem[data-id="' + CSS.escape(ids.worker) + '"]') };
    }, { review: approvalRootId, worker: approvalRootWorkerId });
    r.expect(shownRootPlacement.reviewRoot && shownRootPlacement.workerNested && !shownRootPlacement.workerPromoted, size + " " + theme + ": enabling reviews restores the hidden root and its real child relationship: " + JSON.stringify(shownRootPlacement));
    if (size === "phone" && shownGroup?.all) {
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-all").click();
      await page.waitForFunction(() => document.querySelector("dialog.kids-sheet")?.open === true);
      const shownSheet = await page.locator("dialog.kids-sheet .kids-row").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      r.expect(fanReviewIds.every((id) => shownSheet.includes(id)) && shownSheet.includes(approvalBridgeId), size + " " + theme + ": the enabled sidebar sheet reveals both Fan-out reviews and their worker: " + JSON.stringify(shownSheet));
      await page.screenshot({ path: path.join(ENV.out, "approval-reviews-" + sizeTag + "-" + theme + ".png") });
      await page.locator("dialog.kids-sheet .vclose").click();
      await page.waitForFunction(() => !document.querySelector("dialog.kids-sheet"));
    } else if (shownGroup?.all) {
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-all").click();
      await page.waitForFunction((ids) => ids.every((id) => document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(id) + '"]')), approvalReviewIds);
      const shownIds = await groupOf(page, fan.id);
      r.expect(fanReviewIds.every((id) => shownIds?.ids.includes(id)) && shownIds?.allIds.includes(approvalBridgeId), size + " " + theme + ": the enabled expanded sidebar restores the review nesting and worker: " + JSON.stringify(shownIds?.allIds));
      await page.screenshot({ path: path.join(ENV.out, "approval-reviews-" + sizeTag + "-" + theme + ".png") });
    }
    r.expect(await overflow(page) === 0, size + " " + theme + ": the approval review control and expanded results have no horizontal overflow");

    if (size === "phone") {
      await page.click("#drawer-close");
      await page.waitForFunction(() => !document.body.classList.contains("drawer-open"));
    }
    await page.locator('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]').click();
    await page.waitForFunction(() => document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]')?.getAttribute("aria-pressed") === "false");
    const hiddenAgain = await page.locator("#page .nrow").evaluateAll((rows) => rows.map((x) => x.dataset.id));
    r.expect(!approvalReviewIds.some((id) => hiddenAgain.includes(id)), size + " " + theme + ": the toggle hides reviews again");
    const promotedAgain = await page.evaluate((ids) => ({
      reviewVisible: !!document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(ids.review) + '"]'),
      workerRoot: !!document.querySelector('#lanes > .treeitem[data-id="' + CSS.escape(ids.worker) + '"]'),
    }), { review: approvalRootId, worker: approvalRootWorkerId });
    r.expect(!promotedAgain.reviewVisible && promotedAgain.workerRoot, size + " " + theme + ": disabling reviews promotes the normal child back to a navigation root: " + JSON.stringify(promotedAgain));

    const direct = new URL("/s/codex/" + encodeURIComponent(approvalReviewIds[0]), page.url());
    direct.searchParams.set("t", ENV.extraToken);
    await page.goto(direct.toString(), { waitUntil: "load" }); await settled(page);
    await page.waitForFunction(({ id, parent }) => {
      const row = document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] .srow'), ancestor = document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(parent) + '"]');
      return document.querySelector("#topbar .t")?.textContent === "Approval review" && row?.getAttribute("aria-current") === "page" && ancestor?.getAttribute("aria-expanded") === "true";
    }, { id: approvalReviewIds[0], parent: fan.id });
    r.expect(page.errors.length === 0, size + " " + theme + ": direct review navigation keeps the current review and its expanded parent visible without browser errors: " + JSON.stringify(page.errors));
    r.expect(await overflow(page) === 0, size + " " + theme + ": direct review navigation has no horizontal overflow");
    if (size === "desktop" && !dark) {
      const descendant = new URL("/s/codex/" + encodeURIComponent(approvalRootWorkerId), page.url());
      descendant.searchParams.set("t", ENV.extraToken);
      await page.goto(descendant.toString(), { waitUntil: "load" }); await settled(page);
      await page.waitForFunction(({ id, parent }) => {
        const root = document.querySelector('#lanes > .treeitem[data-id="' + CSS.escape(parent) + '"]'), row = root?.querySelector('.treeitem[data-id="' + CSS.escape(id) + '"] .srow');
        return document.querySelector("#topbar .t")?.textContent === "Root review worker" && root?.getAttribute("aria-expanded") === "true" && row?.getAttribute("aria-current") === "page";
      }, { id: approvalRootWorkerId, parent: approvalRootId });
      r.expect(page.errors.length === 0, "desktop light: a direct descendant URL restores its hidden review ancestor and has no browser errors: " + JSON.stringify(page.errors));
      r.expect(await overflow(page) === 0, "desktop light: the restored root-review path has no horizontal overflow");
    }
    await page.close();
  }

  // An explicit visibility choice discards both held ordering snapshots; unrelated live changes still use the hold behavior.
  {
    const fixture = patchApprovalReviews(structuredClone(D.model), fan.id, fanKids[0].id), newest = Math.max(0, ...Object.values(fixture.sessions).map((s) => Number(s.last) || 0), ...(fixture.handoffs ?? []).map((h) => Number(h.at) || 0));
    const parentOfFixture = (s) => s.parent ?? D.H.find((h) => h.kind === "spawn" && h.to === s.id)?.from;
    const orderRoot = Object.values(D.SESS).filter((s) => s.lane && !parentOfFixture(s) && s.id !== fan.id).sort((a, b) => a.last - b.last)[0];
    r.expect(!!orderRoot, "ordering: the fixture needs an existing top-level session to move on a live update");
    if (orderRoot) {
      const { page, state, baseline } = await servedModel(browser, { extras: true, size: "desktop", dark: false }, (m) => m, D);
      r.expect(await page.locator('#page .groupby[aria-label="Session visibility"]').count() === 0, "ordering: no review toggle is present before review data arrives");
      await page.locator("#lanes .treeitem[data-id=\"" + fan.id + "\"] .tree-all").click();
      await page.waitForFunction((id) => !!document.querySelector('#lanes .treeitem[data-id="' + CSS.escape(id) + '"] .tree-fewer'), fan.id);
      state.edit = (m) => { patchApprovalReviews(m, fan.id, fanKids[0].id); };
      await page.waitForFunction(() => !!document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]'), null, { timeout: 15000 });
      const liveDefault = await page.evaluate(() => ({ count: Number(document.querySelector(".ph .sub span:first-child b")?.textContent), ids: [...document.querySelectorAll("#page .nrow")].map((x) => x.dataset.id) }));
      r.expect(liveDefault.count === baseline.count + 3 && [approvalHelperId, approvalBridgeId, approvalRootWorkerId].every((id) => liveDefault.ids.includes(id)) && !approvalReviewIds.some((id) => liveDefault.ids.includes(id)), "ordering: a live review arrival reveals the control but keeps review rows hidden by default");
      await page.evaluate(() => { const list = document.querySelector("#side-list"); list.scrollTop = Math.min(80, list.scrollHeight - list.clientHeight); });
      const scrollTop = await page.locator("#side-list").evaluate((list) => list.scrollTop);
      r.expect(scrollTop > 1, "ordering: the expanded sidebar is scrolled before the toggle: " + scrollTop);

      const activateToggle = async (value, label) => {
        const row = await page.locator("#page .nrow").first().boundingBox();
        await page.mouse.move(row.x + row.width / 2, row.y + row.height / 2);
        const control = page.locator('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]');
        await control.focus();
        const input = await page.evaluate(() => ({ hover: !!document.querySelector("#page .nrow:hover"), focused: document.activeElement?.matches("[data-show-approval-reviews]") }));
        r.expect(input.hover && input.focused, label + ": the row stays under the pointer while the toggle has keyboard focus: " + JSON.stringify(input));
        await page.keyboard.press("Enter");
        await page.waitForFunction((expected) => document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]')?.getAttribute("aria-pressed") === String(expected), value);
      };
      const expectedOrder = (show, moved = false) => {
        const added = [...approvalFixtureIds]
          .filter((id) => show || fixture.sessions[id].kind !== "Approval review")
          .sort((a, b) => fixture.sessions[b].last - fixture.sessions[a].last);
        return [...(moved ? [orderRoot.id] : []), ...added, ...baseline.ids.filter((id) => !moved || id !== orderRoot.id)];
      };
      const orderStatus = () => page.evaluate(() => ({
        text: document.querySelector("#order-status")?.textContent.trim() ?? "",
        visible: [...document.querySelectorAll('[data-order="page"], [data-order="side"]')].filter((b) => !b.hidden && b.getClientRects().length).length,
      }));

      await activateToggle(true, "ordering enable");
      const enabledIds = await page.locator("#page .nrow").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      const enabledRoots = await page.locator("#lanes > .treeitem").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      const enabledFan = await groupOf(page, fan.id), enabledStatus = await orderStatus();
      r.expect(JSON.stringify(enabledIds) === JSON.stringify(expectedOrder(true)), "ordering enable: reviews enter the Sessions list in last-activity order immediately");
      r.expect(enabledRoots[0] === approvalRootId, "ordering enable: the visible review root takes its recency position: " + JSON.stringify(enabledRoots.slice(0, 3)));
      r.expect(fanReviewIds.every((id) => enabledFan.allIds.includes(id)) && enabledFan.allIds.includes(approvalBridgeId), "ordering enable: expanded tree shows the reviews and bridged worker without opening All N: " + JSON.stringify(enabledFan.allIds));
      r.expect(!enabledStatus.text && enabledStatus.visible === 0, "ordering enable: both order scopes clear held updates: " + JSON.stringify(enabledStatus));

      const movedLast = newest + 100;
      state.edit = (m) => { patchApprovalReviews(m, fan.id, fanKids[0].id); m.sessions[orderRoot.id].last = movedLast; };
      await page.waitForFunction(() => document.querySelector("#order-status")?.textContent.includes("updated"), null, { timeout: 15000 });
      fixture.sessions[orderRoot.id].last = movedLast;
      await activateToggle(false, "ordering disable");
      const disabledIds = await page.locator("#page .nrow").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      const disabledRoots = await page.locator("#lanes > .treeitem").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      const disabledStatus = await orderStatus(), disabledFan = await groupOf(page, fan.id);
      r.expect(JSON.stringify(disabledIds) === JSON.stringify(expectedOrder(false, true)), "ordering disable: the filtered Sessions list re-sorts to last-activity order immediately");
      r.expect(disabledRoots[0] === orderRoot.id && disabledRoots.includes(approvalRootWorkerId) && !disabledRoots.includes(approvalRootId), "ordering disable: the sidebar re-sorts roots and promotes the normal descendant: " + JSON.stringify(disabledRoots.slice(0, 4)));
      r.expect(disabledFan.ids.includes(approvalBridgeId) && !fanReviewIds.some((id) => disabledFan.allIds.includes(id)), "ordering disable: the expanded tree bridges the worker and hides review rows: " + JSON.stringify(disabledFan.allIds));
      r.expect(!disabledStatus.text && disabledStatus.visible === 0, "ordering disable: both order scopes clear the unrelated live-update hold: " + JSON.stringify(disabledStatus));
      await page.close();
    }
  }

  // On a phone, a held order chip remains independently reachable beside a visible approval-review control.
  {
    const fixture = patchApprovalReviews(structuredClone(D.model), fan.id, fanKids[0].id), newest = Math.max(0, ...Object.values(fixture.sessions).map((s) => Number(s.last) || 0), ...(fixture.handoffs ?? []).map((h) => Number(h.at) || 0));
    const parentOfFixture = (s) => s.parent ?? D.H.find((h) => h.kind === "spawn" && h.to === s.id)?.from;
    const orderRoot = Object.values(D.SESS).filter((s) => s.lane && !parentOfFixture(s) && s.id !== fan.id).sort((a, b) => a.last - b.last)[0];
    if (orderRoot) {
      const { page, state, baseline } = await servedModel(browser, { extras: true, size: "phone", dark: false }, (m) => patchApprovalReviews(m, fan.id, fanKids[0].id), D);
      await page.locator('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]').click();
      await page.waitForFunction(() => document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]')?.getAttribute("aria-pressed") === "true");
      const movedLast = newest + 100;
      state.edit = (m) => { m.sessions[orderRoot.id].last = movedLast; };
      await page.waitForFunction(() => document.querySelector("#order-status")?.textContent.includes("updated"), null, { timeout: 15000 });
      fixture.sessions[orderRoot.id].last = movedLast;
      const chipHit = await page.evaluate(() => {
        const chip = document.querySelector('#page .groupby[aria-label="Group by"] .order-chip[data-order="page"]'), review = document.querySelector('#page .groupby[aria-label="Session visibility"] button[data-show-approval-reviews]'), harness = document.querySelector('#page .groupby[aria-label="Group by"] button[data-g="harness"]');
        if (!chip) return null;
        const box = chip.getBoundingClientRect(), target = document.elementFromPoint(box.left + box.width / 2, box.top + box.height / 2), rb = review?.getBoundingClientRect(), hb = harness?.getBoundingClientRect();
        const overlaps = (a, b) => !!a && !!b && a.left < b.right && a.right > b.left && a.top < b.bottom && a.bottom > b.top;
        return { visible: !chip.hidden && !!chip.getClientRects().length, hit: chip.contains(target), reviewOverlap: overlaps(box, rb), harnessOverlap: overlaps(box, hb) };
      });
      r.expect(chipHit?.visible && chipHit.hit && !chipHit.reviewOverlap && !chipHit.harnessOverlap, "phone: the review toggle sits outside the held order chip's hit target: " + JSON.stringify(chipHit));
      await page.locator('#page .groupby[aria-label="Group by"] .order-chip[data-order="page"]').click();
      await page.waitForFunction(() => ![...document.querySelectorAll('[data-order="page"]')].some((b) => !b.hidden && b.getClientRects().length));
      const actual = await page.locator("#page .nrow").evaluateAll((rows) => rows.map((x) => x.dataset.id));
      const added = [...approvalFixtureIds].sort((a, b) => fixture.sessions[b].last - fixture.sessions[a].last);
      const expected = [orderRoot.id, ...added, ...baseline.ids.filter((id) => id !== orderRoot.id)];
      r.expect(JSON.stringify(actual) === JSON.stringify(expected), "phone: activating the order chip clears the page hold and restores last-activity order");
      r.expect(page.errors.length === 0 && await overflow(page) === 0, "phone: order chip clearing with approval reviews has no browser errors or horizontal overflow");
      await page.close();
    }
  }

  // ---- Phone, light: the short lists, the "All N" row, prefs, keys and the sheet --------------------------------------
  {
    const page = await served(browser, { extras: true, size: "phone", dark: false });
    await openDrawer(page);
    const first = await groupOf(page, fan.id), second = await groupOf(page, swarm.id);
    R.phoneFirst = first; R.phoneSwarm = second;
    r.expect(first && JSON.stringify(first.ids) === JSON.stringify(fanShort), "phone: Fan-out lists its running child, then the newest finished: " + JSON.stringify(first?.ids.map(name)) + " expected " + JSON.stringify(fanNames));
    r.expect(first?.expanded === "true", "phone: Fan-out is open by default (a child is running)");
    r.expect(first?.all?.text === "All 7" && first.all.tag === "BUTTON" && first.all.tabIndex === 0 && first.all.role === "treeitem", "phone: one focusable 'All 7' row: " + JSON.stringify(first?.all));
    r.expect(first?.all?.label === "All 7 sessions under Fan-out", "phone: the row's aria-label names the parent: " + first?.all?.label);
    r.expect(first?.pills === 0, "phone: a parent row shows no count pill: " + first?.pills);
    r.expect(first?.all && first.all.height >= 44, "phone: the 'All N' row is at least 44 px tall: " + first?.all?.height);
    r.expect(first?.oldButtons === 0, "phone: no 'Show more' button is left");
    r.expect(first?.stored === null, "phone: nothing is saved until a parent is toggled: " + JSON.stringify(first?.stored));
    r.expect(second && JSON.stringify(second.ids) === JSON.stringify(swarmShort), "phone: Swarm lists its eight newest running workers and no finished one: " + JSON.stringify(second?.ids.map(name)));
    r.expect(second?.all?.text === "All 12" && second.pills === 0, "phone: Swarm ends with 'All 12' and its row shows no count pill: " + JSON.stringify([second?.all?.text, second?.pills]));
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light.png") });

    // Defaults: a parent is open only when something below it is waiting or running (nothing is saved yet, and no session is open).
    const defaults = await page.evaluate(() => [...document.querySelectorAll("#lanes > .treeitem[aria-expanded]")].map((x) => ({ id: x.dataset.id, expanded: x.getAttribute("aria-expanded"), pills: x.querySelectorAll(":scope > .tree-row .tree-summary").length })));
    R.defaults = defaults.map((d) => ({ name: name(d.id), expanded: d.expanded, pills: d.pills }));
    for (const d of defaults) {
      const active = below(d.id).some((k) => k.state === "work" || k.state === "wait");
      r.expect(d.expanded === String(active), "phone: " + name(d.id) + " is " + (d.expanded === "true" ? "open" : "collapsed") + " by default but " + (active ? "has" : "has no") + " running or waiting session below it");
      r.expect(d.pills === 0, "phone: " + name(d.id) + " (" + (d.expanded === "true" ? "open" : "collapsed") + ") shows no count pill: " + d.pills);
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
      // No count pill in a parent's row: not open, not collapsed, and not after collapsing and expanding again.
      const pillCount = () => page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelectorAll(":scope > .tree-row .tree-summary").length, fan.id);
      r.expect(await pillCount() === 0, "desktop: an open parent shows no count pill");
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
      const collapsed = await page.evaluate((id) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id); return { expanded: item.getAttribute("aria-expanded"), pills: item.querySelectorAll(":scope > .tree-row .tree-summary").length }; }, fan.id);
      R.desktopCollapsed = collapsed;
      r.expect(collapsed.expanded === "false" && collapsed.pills === 0, "desktop: a collapsed parent shows no count pill: " + JSON.stringify(collapsed));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-light-collapsed.png") });
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
      await page.waitForTimeout(300);
      r.expect(await pillCount() === 0, "desktop: expanding again shows no count pill either");
    }
    const first = await groupOf(page, fan.id), second = await groupOf(page, swarm.id);
    R.desktopFirst = { fan: first, swarm: second };
    r.expect(first && JSON.stringify(first.ids) === JSON.stringify(fanShort) && first.all?.text === "All 7" && first.pills === 0, "desktop " + tag + ": Fan-out's short list and 'All 7': " + JSON.stringify(first));
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
    // No count pill in any parent's row; the row's accessible label carries the total and breakdown as text. It collapses every
    // parent in turn and saves that, so it runs after the reload assertions.
    {
      const rows = await collapsedRows(page);
      (R.desktopRows ??= {})[tag] = rows;
      assertNoPills(r, "desktop " + tag, rows, D.SESS, D.H, ["Fan-out"]);
    }
    r.expect(page.errors.length === 0, "desktop " + tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- A waiting and a failed run inside a parent (a patched model), desktop and phone: no pill, the label says it ----------
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
      const rows = await collapsedRows(page);
      (R.patchedRows ??= {})[tag] = rows.filter((p) => SESS[p.id]?.name === "Fan-out");
      assertNoPills(r, "patched " + tag, rows, SESS, M.handoffs, ["Fan-out"]);
      const fanRow = rows.find((p) => p.id === fan.id), t = tally(SESS, M.handoffs, fan.id);
      r.expect(t.wait === 1 && t.err === 1 && t.total === fanKids.length + 1, "patched " + tag + ": the patch did not land: " + JSON.stringify(t));
      r.expect(/, 1 needs you/.test(fanRow?.label ?? "") && /, 1 failed/.test(fanRow?.label ?? ""), "patched " + tag + ": Fan-out's label names the waiting and the failed run: " + JSON.stringify(fanRow));
      r.expect(fanRow?.flag.kind === "wait" && fanRow.flagOpen?.kind === "wait" && fanRow.flag.bg === fanRow.want.wait, "patched " + tag + ": Fan-out's dot is amber (--wait-dot) with a run waiting and one failed, open and collapsed: " + JSON.stringify(fanRow));
      // Collapsed for the screenshot.
      await page.evaluate((id) => { const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id); if (item.getAttribute("aria-expanded") === "true") item.querySelector(":scope > .tree-row .tree-toggle").click(); }, fan.id);
      await page.screenshot({ path: path.join(ENV.out, "sidebar-wait-pill-" + tag + ".png") });
      await page.context().close();
    }
    // Only a failed run under Fan-out (nothing waiting): the dot is red, in --err.
    for (const dark of [false, true]) {
      const tag = "red-" + (dark ? "dark" : "light"), red = (m) => { m.sessions[kidIds[1]].state = "err"; return m; }, MR = red(structuredClone(D.model));
      const { page } = await servedModel(browser, { extras: true, size: "desktop", dark }, red);
      await page.waitForSelector("#lanes .treeitem");
      const rows = await collapsedRows(page), fanRow = rows.find((p) => p.id === fan.id), t = tally(MR.sessions, MR.handoffs, fan.id);
      R["redRows" + tag] = rows.filter((p) => MR.sessions[p.id]?.name === "Fan-out");
      r.expect(t.wait === 0 && t.err === 1, tag + ": the patch did not land: " + JSON.stringify(t));
      assertNoPills(r, tag, rows, MR.sessions, MR.handoffs, ["Fan-out"]);
      r.expect(fanRow?.flag.kind === "err" && fanRow.flagOpen?.kind === "err" && fanRow.flag.bg === fanRow.want.err && fanRow.want.err !== fanRow.want.wait, tag + ": Fan-out's dot is red (--err), open and collapsed: " + JSON.stringify(fanRow));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-flag-" + tag + ".png") });
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
      r.expect(start && JSON.stringify(start.ids) === JSON.stringify(fanShort) && start.all?.text === "All 12" && start.pills === 0, "nested " + tag + ": Fan-out counts its 12 descendants in 'All 12' and its row shows no count pill: " + JSON.stringify([start?.ids.map(nname), start?.all?.text, start?.pills]));
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
        // A live update while Swarm's whole list is open: it stays open with its rows where they were and focus on "Show fewer"; the
        // new child (Worker 13) is held and Worker 10's finish is a move, so the sidebar's pill counts them (worked out below from the
        // edited model, by a plain dynamic programme); the pill sits above the parent's stuck row; the pill then reorders the list.
        await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 0; });
        await page.click('#lanes .tree-all[data-id="' + swarm.id + '"]');
        await page.waitForTimeout(150);
        await page.evaluate(() => { document.querySelector("#side-list").scrollTop = 40; }); // the list's top is out of view, so the pill shows
        await page.waitForTimeout(150);
        const held = await groupOf(page, swarm.id);
        state.edit = finishSwarm;
        await page.waitForFunction(() => document.querySelector('.order-pill[data-order="side"]:not([hidden])'), null, { timeout: 15000 });
        await page.waitForTimeout(100);
        const after = await groupOf(page, swarm.id);
        R.liveAfter = after;
        r.expect(after && JSON.stringify(after.ids) === JSON.stringify(held?.ids), "live: the open list keeps its rows in place (Worker 13 is held): " + JSON.stringify(after?.ids.map((id) => nname(id))));
        const post = finishSwarm(nest(structuredClone(D.model))).sessions, kidRank = (x) => (x.state === "wait" ? 0 : x.state === "work" ? 1 : 2), cmp = (a, b) => kidRank(post[a]) - kidRank(post[b]) || post[b].last - post[a].last;
        const dp = (order) => { const d = order.map(() => 1); let best = 0; for (let i = 0; i < order.length; i++) { for (let j = 0; j < i; j++) if (cmp(order[j], order[i]) <= 0) d[i] = Math.max(d[i], d[j] + 1); best = Math.max(best, d[i]); } return order.length - best; };
        const want = 1 + dp(held?.ids ?? []);
        r.expect(await page.evaluate(() => document.querySelector('.order-pill[data-order="side"]')?.textContent.trim()) === want + " updated", "live: the pill counts Worker 13 (held) and the rows that move (" + want + ")");
        r.expect(await page.evaluate(() => { const b = document.querySelector('.order-pill[data-order="side"]'), r = b.getBoundingClientRect(), e = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2); return b.contains(e) && !!document.querySelector("#lanes .tree-row.stuck"); }), "live: the pill isn't on top of the parent's stuck row");
        r.expect(after?.stuck === true && after.fewer === "Show fewer" && after.dialogs === 0, "live: the list is still open and sticky after the update: " + JSON.stringify([after?.stuck, after?.fewer]));
        r.expect(await page.evaluate((id) => { const a = document.activeElement; return a?.classList.contains("tree-fewer") && a.closest(".treeitem").dataset.id === id; }, swarm.id), "live: focus stays on 'Show fewer' through the redraw");
        await page.click('.order-pill[data-order="side"]');
        await page.waitForFunction((id) => document.querySelectorAll('#lanes .treeitem[data-id="' + id + '"] > .tree-group > .treeitem').length === 13, swarm.id, { timeout: 5000 });
        const sorted = await groupOf(page, swarm.id);
        r.expect(sorted && JSON.stringify(sorted.ids) === JSON.stringify(live) && sorted.stuck === true, "live: the pill reorders the open list (Worker 10 finished) and takes Worker 13: " + JSON.stringify(sorted?.ids.map((id) => finishSwarm(nest(structuredClone(D.model))).sessions[id].name)));
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

  // ---- The list scrolls, the rest of the sidebar stays put ----------------------------------------------------------
  const spawned = new Set(D.H.filter((h) => h.kind === "spawn").flatMap((h) => [h.from, h.to]));
  const lone = Object.values(D.SESS).find((s) => s.lane && !s.parent && !spawned.has(s.id));
  r.expect(!!lone, "the extras fixture has no lone lane for the short-list check");
  const oneRow = (m) => {
    m.sessions = { [lone.id]: m.sessions[lone.id] }; m.handoffs = [];
    m.turns = m.turns.filter((t) => t.sid === lone.id); return m;
  };
  for (const [size, tagSize] of [["phone", "390"], ["desktop", "1280"]]) {
    for (const dark of [false, true]) {
      const tag = dark ? "dark" : "light", P = size + " " + tag;
      const page = await served(browser, { extras: true, size, dark });
      await page.waitForSelector("#lanes .treeitem", { state: "attached" });
      if (size === "phone") await openDrawer(page);
      const top = await sideGeometry(page);
      r.expect(top.overflowing, P + ": the fixture's list must overflow or this proves nothing: " + JSON.stringify(top));
      r.expect(top.sidebarTop === 0 && top.listTop === 0 && top.listBottom <= top.vh, P + ": at rest the list ends inside the screen: " + JSON.stringify(top));
      r.expect(!top.allLink, P + ": no 'All sessions' link under the list (the nav's Sessions row goes there): " + JSON.stringify(top));
      r.expect(!top.recentInList && top.recent !== null, P + ": the 'Recent' label sits above the scroller, not inside it: " + JSON.stringify([top.recent, top.recentInList]));
      const atTop = await edges(page);
      R["edges" + P + "top"] = atTop;
      r.expect(!atTop.top && atTop.bottom, P + ": at rest there is no hairline above the rows and one below them: " + JSON.stringify(atTop));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-top.png") });
      for (const where of ["mid", "end"]) {
        await scrollList(page, where);
        const g = await sideGeometry(page);
        R["scroll" + P + where] = g;
        r.expect(g.listTop > 0, P + ": the list scrolled to " + where + ": " + JSON.stringify(g));
        r.expect(g.sidebarTop === 0, P + ": the sidebar itself did not scroll (" + where + "): scrollTop " + g.sidebarTop);
        r.expect(JSON.stringify([g.brand, g.nav]) === JSON.stringify([top.brand, top.nav]), P + ": header and nav kept their boxes (" + where + "): " + JSON.stringify([g.brand, g.nav]) + " vs " + JSON.stringify([top.brand, top.nav]));
        r.expect(g.listBottom === top.listBottom, P + ": the list's foot does not move (" + where + "): " + JSON.stringify(g));
        r.expect(JSON.stringify(g.recent) === JSON.stringify(top.recent), P + ": the 'Recent' label does not move (" + where + "): " + JSON.stringify(g.recent) + " vs " + JSON.stringify(top.recent));
        const e = await edges(page);
        R["edges" + P + where] = e;
        r.expect(e.top && e.bottom === (where === "mid"), P + ": scrolled to " + where + " the hairline shows above the rows" + (where === "mid" ? " and below them" : " and not below them") + ": " + JSON.stringify(e));
        if (where === "mid") await page.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-mid.png") });
      }
      r.expect((await sideGeometry(page)).atEnd, P + ": the list reaches its end");
      // Keyboard focus scrolls the row into the list.
      await scrollList(page, "top");
      await page.evaluate(() => [...document.querySelectorAll("#lanes .srow")].filter((e) => e.getClientRects().length).at(-1).focus());
      const focused = await page.evaluate(() => { const f = document.activeElement.getBoundingClientRect(), l = document.querySelector("#side-list").getBoundingClientRect(); return { top: Math.round(f.top), bottom: Math.round(f.bottom), listTop: Math.round(l.top), listBottom: Math.round(l.bottom), sidebar: document.querySelector("#sidebar").scrollTop }; });
      R["focus" + P] = focused;
      r.expect(focused.bottom <= focused.listBottom && focused.top >= focused.listTop && focused.sidebar === 0, P + ": the last row, focused, sits inside the list: " + JSON.stringify(focused));
      r.expect(await overflow(page) === 0, P + ": no sideways overflow");
      // A one-session model leaves a short list without a sidebar search.
      if (lone) {
        const { page: shortPage } = await servedModel(browser, { extras: true, size, dark }, oneRow);
        if (size === "phone") await openDrawer(shortPage);
        const g = await sideGeometry(shortPage);
        R["short" + P] = g;
        r.expect(g.rows === 1 && !g.overflowing && g.lastRowBottom !== null, P + ": a one-session model leaves one short row: " + JSON.stringify(g));
        r.expect(JSON.stringify(g.brand) === JSON.stringify(top.brand), P + ": the brand row keeps its box with a one-row list: " + JSON.stringify([g.brand, top.brand]));
        r.expect(g.listBottom - g.lastRowBottom >= 0 && g.listBottom - g.lastRowBottom <= 16, P + ": a short list ends right after its last row: gap " + (g.listBottom - g.lastRowBottom));
        const e = await edges(shortPage);
        r.expect(!e.top && !e.bottom, P + ": a short list shows no hairline at either end: " + JSON.stringify(e));
        await shortPage.screenshot({ path: path.join(ENV.out, "sidebar-scroll-" + tagSize + "-" + tag + "-short.png") });
        r.expect(shortPage.errors.length === 0, P + ": short-list page errors " + shortPage.errors.join("; "));
        await shortPage.context().close();
      }
      // The rail: still toggles, no sidebar scroll.
      if (size === "desktop" && !dark) {
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        const g = await sideGeometry(page);
        R.railScroll = g;
        r.expect(await page.evaluate(() => document.querySelector(".app").classList.contains("rail")), "rail: the toggle still collapses the sidebar");
        r.expect(g.sidebarTop === 0, "rail: the sidebar does not scroll: " + JSON.stringify(g));
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        r.expect(!(await page.evaluate(() => document.querySelector(".app").classList.contains("rail"))), "rail: the toggle expands it again");
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

    // A collapse made while the child is open sticks through a rail round-trip.
    await goto(page, { v: "session", id: child.id }, D);
    r.expect((await marks()).expanded === "true", "open: the parent opens again for the next visit to the child");
    await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
    await page.click("#rail-toggle"); await page.waitForTimeout(250);
    await page.click("#rail-toggle"); await page.waitForTimeout(250);
    const stuck = await marks();
    R.collapseSticks = stuck;
    r.expect(stuck.expanded === "false" && stuck.current.length === 1 && stuck.current[0] === child.id, "open: a collapse made while the child is open survives a rail round-trip: " + JSON.stringify(stuck));

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
    const order = (page) => page.evaluate(() => [...document.querySelectorAll("#nav .nav-item")].map((b) => ({ go: b.dataset.go, label: b.querySelector("span:not(.cnt)")?.textContent, cnt: !!b.querySelector(".cnt"), shown: b.getClientRects().length > 0, top: Math.round(b.getBoundingClientRect().top) })));
    const want = ["home", "sessions", "analytics", "machines"], labels = ["Home", "Sessions", "Analytics", "Machines"];
    // The Sessions item carries no count pill (a total there says nothing you act on); it is the only one that never has one.
    const noCount = (o) => o.find((b) => b.go === "sessions")?.cnt === false;
    const ok = (o) => JSON.stringify(o.map((b) => b.go)) === JSON.stringify(want) && JSON.stringify(o.map((b) => b.label)) === JSON.stringify(labels) && o.every((b) => b.shown) && o.every((b, i) => i === 0 || b.top > o[i - 1].top) && noCount(o);
    const desktop = await served(browser, { extras: true, size: "desktop", dark: false });
    await desktop.waitForSelector("#nav .nav-item");
    const expanded = await order(desktop);
    R.navExpanded = expanded;
    r.expect(ok(expanded), "nav: the expanded sidebar lists Home, Sessions, Analytics, Machines top to bottom: " + JSON.stringify(expanded));
    await desktop.click("#rail-toggle"); await desktop.waitForTimeout(300);
    const rail = await order(desktop);
    R.navRail = rail;
    r.expect(rail.map((b) => b.go).join() === want.join() && rail.every((b) => b.shown) && rail.every((b, i) => i === 0 || b.top > rail[i - 1].top) && noCount(rail), "nav: the rail's icons run Home, Sessions, Analytics, Machines top to bottom, and Sessions has no count: " + JSON.stringify(rail));
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

  // ---- The header: the logo first, the toggle (or the drawer's close button) at the right ---------------------------
  const headBoxes = (page, control) => page.evaluate((control) => {
    const box = (sel) => { const e = document.querySelector(sel); if (!e || !e.getClientRects().length) return null; const b = e.getBoundingClientRect(); return { left: Math.round(b.left * 10) / 10, right: Math.round(b.right * 10) / 10, top: Math.round(b.top * 10) / 10, bottom: Math.round(b.bottom * 10) / 10, w: Math.round(b.width * 10) / 10, h: Math.round(b.height * 10) / 10 }; };
    const mark = document.querySelector(".brandrow .mark"), ctl = document.querySelector(control);
    return { mark: box(".brandrow .mark"), name: box(".brandrow .brandname"), control: box(control), sidebar: box("#sidebar"), nav: box("#nav"), markFirst: !!(mark && ctl && (mark.compareDocumentPosition(ctl) & Node.DOCUMENT_POSITION_FOLLOWING)), label: ctl?.getAttribute("aria-label"), expanded: ctl?.getAttribute("aria-expanded") };
  }, control);
  const railBackground = (page) => page.evaluate(() => {
    const toggle = document.querySelector("#rail-toggle"), sample = document.createElement("i");
    sample.style.background = "var(--sunken)"; document.body.append(sample);
    const background = getComputedStyle(toggle).backgroundColor, sunken = getComputedStyle(sample).backgroundColor;
    sample.remove(); return { background, sunken };
  });
  const pointerAway = async (page) => { const viewport = page.viewportSize(); await page.mouse.move(viewport.width - 2, viewport.height - 2); };
  for (const [size, tagSize, control] of [["desktop", "1280", "#rail-toggle"], ["phone", "390", "#drawer-close"]]) {
    for (const dark of [false, true]) {
      const tag = dark ? "dark" : "light", P = "header " + tagSize + " " + tag;
      const page = await served(browser, { extras: true, size, dark });
      await page.waitForSelector("#lanes .treeitem", { state: "attached" });
      if (size === "phone") await openDrawer(page);
      const h = await headBoxes(page, control);
      R[P] = h;
      r.expect(h.mark && h.control && h.mark.left < h.control.left && h.markFirst, P + ": the logo comes before the control: " + JSON.stringify(h));
      r.expect(h.control && h.sidebar.right - h.control.right >= 0 && h.sidebar.right - h.control.right <= 16, P + ": the control is at the right end of the header, within 16px of the sidebar's edge: " + JSON.stringify(h));
      r.expect(h.control && h.control.w >= 44 && h.control.h >= 44, P + ": the control is at least 44x44: " + JSON.stringify(h.control));
      r.expect(h.mark && h.mark.left >= h.sidebar.left && h.name && h.name.right <= h.control.left, P + ": the logo and name fit left of the control: " + JSON.stringify(h));
      r.expect(h.label === (size === "phone" ? "Close menu" : "Collapse sidebar") && (size === "phone" || h.expanded === "true"), P + ": label and state: " + h.label + " / " + h.expanded);
      if (size === "desktop") {
        await pointerAway(page);
        const expandedAway = await railBackground(page);
        r.expect(expandedAway.background === "rgba(0, 0, 0, 0)", P + ": the expanded rail toggle has a transparent background with the pointer away: " + JSON.stringify(expandedAway));
        await page.locator("#rail-toggle").hover();
        const expandedHover = await railBackground(page);
        r.expect(expandedHover.background === expandedHover.sunken, P + ": the expanded rail toggle uses --sunken on hover: " + JSON.stringify(expandedHover));
        await pointerAway(page);
        const next = page.locator("#sidebar #nav .nav-item").first();
        await next.focus();
        await page.keyboard.press("Shift+Tab");
        const focus = await page.evaluate(() => {
          const active = document.activeElement, style = getComputedStyle(active);
          return { id: active.id, visible: active.matches(":focus-visible"), outlineStyle: style.outlineStyle, outlineWidth: style.outlineWidth };
        });
        r.expect(focus.id === "rail-toggle" && focus.visible && focus.outlineStyle === "solid" && focus.outlineWidth === "2px", P + ": Shift+Tab from the first nav item reaches the toggle, with a focus ring: " + JSON.stringify(focus));
        await page.keyboard.press("Tab");
        r.expect(await page.evaluate(() => document.activeElement === document.querySelector("#sidebar #nav .nav-item")), P + ": Tab from the toggle returns to the first nav item");
      }
      await page.screenshot({ path: path.join(ENV.out, "sidebar-header-" + tagSize + "-" + tag + ".png") });
      if (size === "desktop") {
        // The rail: the toggle shows, the logo mark sits above it, and both fit the 64px column.
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        const rail = await headBoxes(page, "#rail-toggle");
        R[P + " rail"] = rail;
        r.expect(rail.control && rail.control.w >= 44 && rail.control.h >= 44 && rail.control.left >= rail.sidebar.left && rail.control.right <= rail.sidebar.right, P + " rail: the toggle shows at least 44x44 inside the rail: " + JSON.stringify(rail));
        r.expect(rail.mark && rail.mark.bottom <= rail.control.top && rail.mark.left >= rail.sidebar.left && rail.mark.right <= rail.sidebar.right && rail.control.bottom <= rail.nav.top, P + " rail: the logo mark sits above the toggle, clear of the nav: " + JSON.stringify(rail));
        r.expect(rail.label === "Expand sidebar" && rail.expanded === "false" && rail.name === null, P + " rail: label and state: " + rail.label + " / " + rail.expanded);
        await pointerAway(page);
        const collapsedAway = await railBackground(page);
        r.expect(collapsedAway.background === "rgba(0, 0, 0, 0)", P + " rail: the collapsed toggle has a transparent background with the pointer away: " + JSON.stringify(collapsedAway));
        await page.locator("#rail-toggle").hover();
        const collapsedHover = await railBackground(page);
        r.expect(collapsedHover.background === collapsedHover.sunken, P + " rail: the collapsed toggle uses --sunken on hover: " + JSON.stringify(collapsedHover));
        await pointerAway(page);
        await page.screenshot({ path: path.join(ENV.out, "sidebar-header-" + tagSize + "-" + tag + "-rail.png") });
        await page.click("#rail-toggle"); await page.waitForTimeout(300);
        const back = await headBoxes(page, "#rail-toggle");
        r.expect(back.label === "Collapse sidebar" && back.name && back.mark.left < back.control.left, P + ": the toggle expands the rail again");
        await pointerAway(page);
        const expandedBackAway = await railBackground(page);
        r.expect(expandedBackAway.background === "rgba(0, 0, 0, 0)", P + ": the expanded toggle remains transparent after returning from the rail: " + JSON.stringify(expandedBackAway));
      } else {
        r.expect(!(await headBoxes(page, "#rail-toggle")).control, P + ": the rail toggle stays hidden in the drawer");
        if (lone) {
          const { page: shortPage } = await servedModel(browser, { extras: true, size, dark }, oneRow);
          await openDrawer(shortPage);
          const short = await headBoxes(shortPage, control);
          R[P + " short"] = short;
          r.expect(short.mark && short.mark.top < 60 && JSON.stringify([short.mark, short.name, short.control, short.nav]) === JSON.stringify([h.mark, h.name, h.control, h.nav]) && short.nav.top >= short.control.bottom && short.nav.top - short.control.bottom <= 24, P + ": with a one-row list the brand stays at the top and the nav right below it: " + JSON.stringify(short));
          await shortPage.screenshot({ path: path.join(ENV.out, "sidebar-header-" + tagSize + "-" + tag + "-short.png") });
          r.expect(shortPage.errors.length === 0, P + ": short-header page errors " + shortPage.errors.join("; "));
          await shortPage.context().close();
        }
        await page.click("#drawer-close"); await page.waitForTimeout(300);
        r.expect(await page.evaluate(() => !document.body.classList.contains("drawer-open")), P + ": the close button closes the drawer");
      }
      r.expect(await overflow(page) === 0, P + ": no sideways overflow");
      r.expect(page.errors.length === 0, P + ": page errors " + page.errors.join("; "));
      await page.context().close();
    }
  }

  // ---- An expanded top-level parent stays pinned while its children scroll (phone drawer and desktop sidebar, light and dark) --------
  // The model is patched so two parents overflow the list on their own: Swarm gets four grandchildren (under Workers 10 to 7), and a copy
  // of it, "Swarm B", gets the same. Scrolling into each group then shows its row pinned at the list's top, the topmost thing there, with
  // its children under it; past the group the next parent takes over. Nested parents and collapsed ones do not pin.
  {
    const runningKids = swarmKids.filter((c) => c.state === "work"), topKids = runningKids.slice(0, 4);
    const stickyPatch = (m) => {
      const spawnTo = (to) => m.handoffs.find((h) => h.kind === "spawn" && h.to === to);
      const put = (src, id, nm, parent, extra = {}) => {
        m.sessions[id] = { ...structuredClone(m.sessions[src]), id, name: nm, parent, ...extra };
        const sp = spawnTo(src); if (sp) m.handoffs.push({ ...structuredClone(sp), id: id + "-spawn", from: parent, to: id });
      };
      const b = "sticky-swarm-b";
      m.sessions[b] = { ...structuredClone(m.sessions[swarm.id]), id: b, name: "Swarm B", last: m.sessions[swarm.id].last - 1 };
      for (const k of runningKids) put(k.id, "sticky-b-" + k.id, k.name, b);
      for (const [i, k] of topKids.entries()) {
        put(k.id, "sticky-gc-a-" + i, "Helper " + (i + 1), k.id, { state: "work" });
        put(k.id, "sticky-gc-b-" + i, "Helper " + (i + 1), "sticky-b-" + k.id, { state: "work" });
      }
      return m;
    };
    const layout = (page) => page.evaluate(() => {
      const list = document.querySelector("#side-list"), lr = list.getBoundingClientRect();
      return {
        client: list.clientHeight, max: list.scrollHeight - list.clientHeight,
        tops: [...document.querySelectorAll("#lanes > .treeitem")].map((x) => { const row = x.querySelector(":scope > .tree-row"), b = x.getBoundingClientRect(); return { id: x.dataset.id, name: x.getAttribute("aria-label"), open: x.getAttribute("aria-expanded") === "true", kids: !!x.querySelector(":scope > .tree-group"), top: b.top - lr.top + list.scrollTop, height: b.height, position: getComputedStyle(row).position }; }),
        nested: [...document.querySelectorAll("#lanes .tree-group .treeitem[aria-expanded='true'] > .tree-row")].map((row) => getComputedStyle(row).position),
      };
    });
    // Where the item's row is, what is topmost at the list's top edge and just under the row, and whether the chevron can be tapped.
    const probe = (page, id) => page.evaluate((id) => {
      const list = document.querySelector("#side-list"), lr = list.getBoundingClientRect();
      const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id), row = item.querySelector(":scope > .tree-row"), rr = row.getBoundingClientRect();
      const x = rr.left + Math.min(40, rr.width / 2), atTop = document.elementFromPoint(x, lr.top + 2), under = document.elementFromPoint(x, lr.top + rr.height + 8);
      const t = row.querySelector(".tree-toggle"), tb = t?.getBoundingClientRect(), hit = tb ? document.elementFromPoint(tb.left + tb.width / 2, tb.top + tb.height / 2) : null;
      const cs = getComputedStyle(row), side = getComputedStyle(document.querySelector("#sidebar")).backgroundColor;
      return {
        rowTop: Math.round((rr.top - lr.top) * 10) / 10, rowH: Math.round(rr.height * 10) / 10, position: cs.position, topmost: !!atTop && row.contains(atTop),
        underIsChild: !!under && item.querySelector(":scope > .tree-group").contains(under), bg: cs.backgroundColor, side, scrollTop: Math.round(list.scrollTop),
        toggleTappable: !!t && !!hit && (hit === t || t.contains(hit)) && tb.top >= lr.top - 0.5 && tb.bottom <= lr.bottom + 0.5, aboveList: rr.bottom <= lr.top + 0.5,
      };
    }, id);
    for (const [size, tagSize, viewport] of [["phone", "390", null], ["desktop", "1280", { width: 1280, height: 800 }]]) {
      for (const dark of [false, true]) {
        const tag = dark ? "dark" : "light", P = "sticky parent " + tagSize + " " + tag;
        const { page } = await servedModel(browser, { extras: true, size, dark }, stickyPatch);
        if (viewport) { await page.setViewportSize(viewport); await page.waitForTimeout(150); }
        if (size === "phone") await openDrawer(page);
        await page.waitForFunction(() => document.querySelectorAll("#lanes > .treeitem[aria-expanded='true']").length >= 2);
        const L = await layout(page);
        R[P + " layout"] = L;
        const tall = L.tops.filter((t) => t.open && t.height >= L.client + 60);
        r.expect(tall.length >= 2, P + ": the patched fixture needs two open parents taller than the list (" + L.client + " px), so the check proves nothing: " + JSON.stringify(L.tops));
        r.expect(L.tops.some((t) => t.kids && !t.open) && L.tops.filter((t) => t.kids && !t.open).every((t) => t.position !== "sticky"), P + ": a collapsed parent's row does not stick: " + JSON.stringify(L.tops));
        r.expect(L.nested.length >= 1 && L.nested.every((p) => p !== "sticky"), P + ": a nested parent's row does not stick (only the top level does): " + JSON.stringify(L.nested));
        if (tall.length >= 2) {
          const [a, b] = tall;
          const check = async (which, want, other) => {
            const p = await probe(page, want.id);
            R[P + " " + which] = p;
            r.expect(p.position === "sticky", P + " " + which + ": " + want.name + "'s row is position: sticky: " + JSON.stringify(p));
            r.expect(Math.abs(p.rowTop) <= 1, P + " " + which + ": " + want.name + "'s row top equals the list's top: " + JSON.stringify(p));
            r.expect(p.topmost, P + " " + which + ": " + want.name + "'s row is the topmost element at the list's top: " + JSON.stringify(p));
            r.expect(p.underIsChild, P + " " + which + ": its children are under the row: " + JSON.stringify(p));
            r.expect(p.toggleTappable, P + " " + which + ": the chevron is in view and takes the tap: " + JSON.stringify(p));
            r.expect(p.bg === p.side && /^rgb\(/.test(p.bg), P + " " + which + ": the row is opaque, in the sidebar's colour: " + p.bg + " vs " + p.side);
            if (other) { const o = await probe(page, other.id); r.expect(o.aboveList, P + " " + which + ": " + other.name + "'s row has gone with its group: " + JSON.stringify(o)); }
          };
          await scrollSidebar(page, a.top + 30);
          await check("first", a, null);
          await page.screenshot({ path: path.join(ENV.out, "sidebar-sticky-" + tagSize + "-" + tag + "-first.png") });
          // Focus lands clear of the pinned row: the first child starts under it and is scrolled to below the row.
          const kid = await page.evaluate((id) => {
            const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id), k = item.querySelector(":scope > .tree-group > .treeitem .srow");
            k.focus();
            return new Promise((res) => setTimeout(() => { const kr = k.getBoundingClientRect(), rr = item.querySelector(":scope > .tree-row").getBoundingClientRect(); res({ kidTop: Math.round(kr.top * 10) / 10, rowBottom: Math.round(rr.bottom * 10) / 10, focused: document.activeElement === k }); }, 150));
          }, a.id);
          R[P + " focus"] = kid;
          r.expect(kid.focused && kid.kidTop >= kid.rowBottom - 1, P + ": a focused child is scrolled clear of the pinned row: " + JSON.stringify(kid));
          await scrollSidebar(page, Math.min(b.top + 30, L.max));
          await check("second", b, a);
          await page.screenshot({ path: path.join(ENV.out, "sidebar-sticky-" + tagSize + "-" + tag + "-second.png") });
          await scrollSidebar(page, 0);
        }
        r.expect(await overflow(page) === 0, P + ": no sideways overflow");
        r.expect(page.errors.length === 0, P + ": page errors " + page.errors.join("; "));
        await page.context().close();
      }
    }
  }
  return r.done();
}
