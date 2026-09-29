// The sidebar tree, on the extras fixture (it holds `Fan-out`: a live parent with seven subagents, the oldest still
// running), phone and desktop:
//   - a parent lists its five newest children plus any that is working or waiting, then one "Show N more" button that
//     names the parent; the running oldest child shows, so the fixture's N is 1.
//   - the button reveals the rest and becomes "Show fewer"; the choice is saved per parent, beside `open`, and survives a
//     reload; "Show fewer" folds them again.
//   - while a search is typed every child shows and there is no button.
//   - the toggle sits over the right end of its row's meta line, and only rows with children have one: no row has a left
//     gutter, so within a group every name starts at the same x and every time ends at the same x whether or not the row has
//     children, and no `.tree-spacer` is left.
//   - arrow keys act on their own row: ArrowLeft on "Show N more" or on a leaf does not collapse its parent.
//   - the toggle's box is at least 44x44 at 390 px and at least 28x36 at 1280 px, and a collapsed parent's summary does
//     not overlap it.
//   - only the open session's row is current: its parent is not highlighted; a collapsed parent opens for the child's page
//     without saving that, and its saved choice is unchanged after navigating away; a collapse made while the child is open
//     stays collapsed through redraws; the open child is never folded into 'Show N more'; in the rail the ancestor keeps a
//     ring, no highlight, and aria-current="true".
//   - screenshots of the sidebar at 390 and 1280, light and dark, go to out/sidebar-*.png.
import path from "node:path";
import { ENV, served, goto, data, reporter, overflow } from "../lib.mjs";

const openDrawer = async (page) => { await page.click("#lead-btn"); await page.waitForTimeout(300); };

// The direct children of a parent's group, and its button.
const groupOf = (page, id) => page.evaluate((id) => {
  const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id);
  if (!item) return null;
  const button = item.querySelector(":scope > .tree-group > .tree-more");
  const stored = JSON.parse(localStorage.getItem("semon.tree") ?? "{}")[id] ?? null;
  return {
    ids: [...item.querySelectorAll(":scope > .tree-group > .treeitem")].map((x) => x.dataset.id),
    more: button ? { text: button.textContent, role: button.getAttribute("role"), label: button.getAttribute("aria-label"), expanded: button.getAttribute("aria-expanded"), tag: button.tagName, tabIndex: button.tabIndex } : null,
    expanded: item.getAttribute("aria-expanded"), stored,
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

export default async function sidebarCheck(browser) {
  const D = await data({ extras: true });
  const r = reporter("sidebar");
  const R = r.results;
  const fan = Object.values(D.SESS).find((s) => s.name === "Fan-out");
  r.expect(!!fan, "the extras fixture has no Fan-out session");
  if (!fan) return r.done();
  const kids = D.H.filter((h) => h.kind === "spawn" && h.from === fan.id).map((h) => D.SESS[h.to]).filter(Boolean).sort((a, b) => b.last - a.last);
  const urgent = (c) => c.state === "work" || c.state === "wait";
  const shownIds = kids.filter((c, i) => i < 5 || urgent(c)).map((c) => c.id), hidden = kids.length - shownIds.length;
  r.expect(kids.length >= 7, "Fan-out needs at least 7 children, has " + kids.length);
  r.expect(kids.findIndex(urgent) >= 5, "the fixture's running child must be older than the 5 newest: states " + kids.map((c) => c.state).join(","));
  r.expect(hidden >= 1, "nothing is hidden by the cap: " + hidden);
  R.expected = { kids: kids.length, shown: shownIds.length, hidden, states: kids.map((c) => c.state) };

  // ---- Phone, light: the cap, the button, the saved choice, search, the toggle's box --------------------------------
  {
    const page = await served(browser, { extras: true, size: "phone", dark: false });
    await openDrawer(page);
    const first = await groupOf(page, fan.id);
    R.phoneFirst = first;
    r.expect(first && JSON.stringify(first.ids) === JSON.stringify(shownIds), "phone: the group shows the 5 newest and the running child in newest-first order: " + JSON.stringify(first?.ids) + " expected " + JSON.stringify(shownIds));
    r.expect(first?.expanded === "true", "phone: Fan-out is open by default (a child is running)");
    r.expect(first?.more?.text === "Show " + hidden + " more" && first.more.tag === "BUTTON" && first.more.tabIndex === 0, "phone: one focusable 'Show N more' button: " + JSON.stringify(first?.more));
    r.expect(first?.more?.label?.includes("Fan-out") && first.more.label.startsWith("Show " + hidden + " more " + (hidden === 1 ? "session spawned by" : "sessions spawned by")), "phone: the button's aria-label agrees in number and names the parent: " + first?.more?.label);
    r.expect(first?.more?.role === "treeitem", "phone: the button is reachable in tree navigation (role treeitem): " + first?.more?.role);
    r.expect(first?.stored?.more === undefined, "phone: nothing is saved until the button is used");
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light.png") });

    const box = await toggleBox(page, fan.id);
    R.phoneToggle = box;
    r.expect(box && box.w >= 44 && box.h >= 44, "phone: the toggle is at least 44x44: " + JSON.stringify(box));
    r.expect(box?.right && box.lower, "phone: the toggle sits over the right end of the row's meta line: " + JSON.stringify(box));
    r.expect(box?.color === "rgb(111, 119, 115)", "phone: the chevron is --muted (4.29:1 on the light sidebar): " + box?.color);
    r.expect(box?.bg === "rgba(0, 0, 0, 0)", "phone: the toggle has no background until hover or focus: " + box?.bg);
    const g = await gutters(page);
    R.phoneGutters = g;
    r.expect(g.spacers === 0, "phone: no .tree-spacer remains");
    r.expect(g.mixed >= 1, "phone: no group mixes rows with and without children, so the gutter check proves nothing");
    r.expect(g.groups.every((x) => x.lefts.length === 1), "phone: names in a group start at different x: " + JSON.stringify(g.groups));
    r.expect(g.groups.every((x) => x.ends.length === 1), "phone: times in a group end at different x: " + JSON.stringify(g.groups));

    await page.click('#lanes .tree-more[aria-label*="Fan-out"]');
    const revealed = await groupOf(page, fan.id);
    R.phoneRevealed = revealed;
    r.expect(revealed && JSON.stringify(revealed.ids) === JSON.stringify(kids.map((c) => c.id)), "phone: the button reveals every child, newest first: " + JSON.stringify(revealed?.ids));
    r.expect(revealed?.more?.text === "Show fewer" && revealed.more.expanded === "true", "phone: the button becomes 'Show fewer': " + JSON.stringify(revealed?.more));
    r.expect(revealed?.stored?.more === true && typeof revealed.stored.at === "number", "phone: the choice is saved per parent: " + JSON.stringify(revealed?.stored));
    r.expect(await page.evaluate(() => document.activeElement?.classList.contains("tree-more")), "phone: focus stays on the button after it is used");
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-light-more.png") });

    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#lanes .treeitem"));
    const reloaded = await groupOf(page, fan.id);
    R.phoneReloaded = reloaded;
    r.expect(reloaded && reloaded.ids.length === kids.length && reloaded.more?.text === "Show fewer", "phone: the revealed children survive a reload: " + JSON.stringify(reloaded));

    // A saved value from before this change holds only `open`: it still works and keeps its choice.
    await page.evaluate((id) => { const t = JSON.parse(localStorage.getItem("semon.tree") ?? "{}"); t[id] = { open: true, at: Date.now() }; localStorage.setItem("semon.tree", JSON.stringify(t)); }, fan.id);
    await page.reload({ waitUntil: "load" });
    await page.waitForFunction(() => document.querySelector("#lanes .treeitem"));
    const legacy = await groupOf(page, fan.id);
    r.expect(legacy && legacy.expanded === "true" && legacy.ids.length === shownIds.length && legacy.more?.text === "Show " + hidden + " more", "phone: an older saved {open} value still loads: " + JSON.stringify(legacy));

    // Search: every child shows, and there is no button.
    await openDrawer(page);
    await page.fill("#q", "Reader"); await page.waitForTimeout(120);
    const searching = await groupOf(page, fan.id);
    R.phoneSearch = searching;
    r.expect(searching && searching.ids.length === kids.length && !searching.more, "phone: a search shows every child with no button: " + JSON.stringify(searching));
    await page.fill("#q", ""); await page.waitForTimeout(120);
    const cleared = await groupOf(page, fan.id);
    r.expect(cleared && cleared.ids.length === shownIds.length && cleared.more, "phone: clearing the search caps the group again");

    // Arrow keys act on their own row: on "Show N more" and on a leaf they leave the parent open and save nothing.
    await page.focus('#lanes .tree-more[aria-label*="Fan-out"]'); await page.keyboard.press("ArrowLeft");
    const arrowMore = await groupOf(page, fan.id);
    r.expect(arrowMore?.expanded === "true" && arrowMore.stored?.open !== false, "phone: ArrowLeft on 'Show N more' leaves the parent open: " + JSON.stringify(arrowMore));
    await page.evaluate((id) => [...document.querySelectorAll("#lanes .treeitem")].find((x) => x.dataset.id === id).focus(), shownIds[0]);
    await page.keyboard.press("ArrowLeft");
    const arrowLeaf = await groupOf(page, fan.id);
    r.expect(arrowLeaf?.expanded === "true" && arrowLeaf.stored?.open !== false, "phone: ArrowLeft on a leaf leaves its parent open: " + JSON.stringify(arrowLeaf));

    // Show fewer folds them, and saves that.
    await page.click('#lanes .tree-more[aria-label*="Fan-out"]'); await page.waitForTimeout(60);
    await page.click('#lanes .tree-more[aria-label*="Fan-out"]'); await page.waitForTimeout(60);
    const folded = await groupOf(page, fan.id);
    r.expect(folded && folded.ids.length === shownIds.length && folded.more?.text === "Show " + hidden + " more" && folded.stored?.more === false, "phone: 'Show fewer' folds the group and saves it: " + JSON.stringify(folded));
    r.expect(await overflow(page) === 0, "phone: the sidebar has no sideways overflow");
    r.expect(page.errors.length === 0, "phone: page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- Phone, dark: a screenshot ------------------------------------------------------------------------------------
  {
    const page = await served(browser, { extras: true, size: "phone", dark: true });
    await openDrawer(page);
    await page.screenshot({ path: path.join(ENV.out, "sidebar-390-dark.png") });
    r.expect(page.errors.length === 0, "phone dark: page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- Desktop: the toggle's hit area, the summary, the gutter ------------------------------------------------------
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
      const first = await groupOf(page, fan.id);
      r.expect(first && JSON.stringify(first.ids) === JSON.stringify(shownIds) && first.more?.text === "Show " + hidden + " more", "desktop: the cap and the button: " + JSON.stringify(first));
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
      r.expect(collapsed?.expanded === "false" && collapsed.summary === String(kids.length) && collapsed.gap >= 0, "desktop: the collapsed summary clears the toggle: " + JSON.stringify(collapsed));
      await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-light-collapsed.png") });
      await page.evaluate((id) => [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === id).querySelector(":scope > .tree-row .tree-toggle").click(), fan.id);
      await page.waitForTimeout(300);
      r.expect(!(await summaryShown()), "desktop: expanding hides the child count again");
    }
    await page.screenshot({ path: path.join(ENV.out, "sidebar-1280-" + tag + ".png") });
    r.expect(page.errors.length === 0, "desktop " + tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }

  // ---- The open session's row is the only one marked ----------------------------------------------------------------
  {
    // Reader 2 is the child the cap folds away when nothing is open.
    const child = kids.find((c) => !shownIds.includes(c.id)) ?? kids[kids.length - 1];
    const page = await served(browser, { extras: true, size: "desktop", dark: false });
    await page.waitForSelector("#lanes .treeitem");
    const marks = () => page.evaluate(({ parent, child }) => {
      const cur = [...document.querySelectorAll("#lanes [aria-current='page']")].map((x) => x.dataset.id);
      const item = [...document.querySelectorAll("#lanes > .treeitem")].find((x) => x.dataset.id === parent);
      const pick = (id) => item?.querySelector('.srow[data-id="' + CSS.escape(id) + '"]') ?? (id === parent ? item?.querySelector(":scope > .tree-row .srow") : null);
      return { current: cur, expanded: item?.getAttribute("aria-expanded"), childShown: !!pick(child), onPath: !!item?.querySelector(":scope > .tree-row .srow.on-path"), ring: item?.querySelector(":scope > .tree-row .srow.on-path")?.getAttribute("aria-current") ?? null, ringColor: item?.querySelector(":scope > .tree-row .srow.on-path") ? getComputedStyle(item.querySelector(":scope > .tree-row .srow.on-path")).boxShadow : null, more: item?.querySelector(":scope > .tree-group > .tree-more")?.textContent ?? null,
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
    r.expect(open.more === null || !open.more.includes("Show fewer"), "open: the open child does not force the group to 'Show fewer': " + open.more);
    const saved = await pref();
    r.expect(saved?.open === false && saved.at === 1 && saved.more === undefined, "open: the parent's saved choice is unchanged while its child is open: " + JSON.stringify(saved));
    r.expect(open.childBg !== "rgba(0, 0, 0, 0)" && open.parentBg === "rgba(0, 0, 0, 0)", "open: only the open child is highlighted: parent " + open.parentBg + ", child " + open.childBg);
    r.expect(open.more === null && open.childShown, "open: the open child counts as shown under the cap, so nothing folds away: " + open.more);
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
    r.expect(rail.ringColor?.startsWith("rgb(111, 119, 115)"), "rail: the ring is --muted, not --faint: " + rail.ringColor);
    r.expect(page.errors.length === 0, "open: page errors " + page.errors.join("; "));
    await page.context().close();
  }
  return r.done();
}
