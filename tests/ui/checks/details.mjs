// The Session details sheet reads as one list: the API-equivalent cost row is a plain row like the ones above it (no button chrome,
// no tap), its breakdown is always shown as a quiet table (kind, tokens, cost) per model, kinds that used and cost nothing are
// left out, and no label wraps at 390 px. Opened on the session with the most cost detail, and on the extras fixture's web-search
// session (a priced kind with no token count: its tokens cell is empty), at 390 and 1280 px, light and dark; screenshots go to
// out/details/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "details");
fs.mkdirSync(OUT, { recursive: true });
const near = (a, b) => Math.abs(a - b) <= 1;

// Reads the open sheet. A cell's text is measured on its own text node (the tooltip's screen-reader text sits beside it), and an
// empty cell has none.
const read = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-details"), lines = (n) => { const cs = getComputedStyle(n), lh = parseFloat(cs.lineHeight) || parseFloat(cs.fontSize) * 1.25; return Math.round(n.getBoundingClientRect().height / lh); };
  const row = d.querySelector(".cost-row"), cs = row && getComputedStyle(row), plain = d.querySelector(".detail-row:not(.cost-row)");
  const r1 = (v) => Math.round(v * 10) / 10;
  const cell = (c) => {
    const node = c.firstChild, text = node?.nodeType === 3 && node.length ? node : null;
    if (!text) return { text: "", tip: c.dataset.tip ?? null };
    const range = document.createRange(); range.selectNodeContents(text); const t = range.getBoundingClientRect();
    return { text: text.data, tip: c.dataset.tip ?? null, base: r1(t.bottom), textRight: r1(t.right) };
  };
  const groups = [...d.querySelectorAll(".cost-model")].map((g) => ({
    model: g.querySelector(".cost-model-name")?.textContent,
    head: [...g.querySelector(".cost-model-head").children].map(cell),
    rows: [...g.querySelectorAll(".cost-line")].map((l) => { const cells = [...l.children].map(cell); return { cells, kind: cells[0].text, tokens: cells[1].text, cost: cells[2].text }; }),
  }));
  const dr = d.getBoundingClientRect(), vb = d.querySelector(".vb");
  return {
    tag: row?.tagName, background: cs?.backgroundColor, expanded: row?.hasAttribute("aria-expanded"),
    rowSameGrid: !!row && !!plain && cs.display === getComputedStyle(plain).display && cs.paddingTop === getComputedStyle(plain).paddingTop && cs.borderBottomWidth === getComputedStyle(plain).borderBottomWidth,
    valueLeft: [...d.querySelectorAll(".detail-row")].map((x) => Math.round(x.querySelector(".detail-value").getBoundingClientRect().left)),
    labels: [...d.querySelectorAll(".detail-label")].map((n) => ({ text: n.textContent, lines: lines(n) })), groups,
    hidden: !!d.querySelector(".cost-breakdown")?.hidden, breakdownVisible: (d.querySelector(".cost-breakdown")?.getClientRects().length ?? 0) > 0,
    tipIcon: !!d.querySelector(".cost-breakdown-head .cost-info[data-tip]"), sideways: vb.scrollWidth > vb.clientWidth + 1 || document.documentElement.scrollWidth > document.documentElement.clientWidth, inView: dr.left >= 0 && dr.right <= innerWidth + 0.5,
    fonts: { row: parseFloat(getComputedStyle(d.querySelector(".cost-line")).fontSize), detail: parseFloat(getComputedStyle(d.querySelector(".detail-value")).fontSize), name: parseFloat(getComputedStyle(d.querySelector(".cost-model-name")).fontSize) },
  };
});

export default async function detailsCheck(browser) {
  const D = await data(), X = await data({ extras: true }), r = reporter("details"), results = {};
  // The session that reports the most cost lines: the richest sheet the fixture can show.
  const sid = Object.values(D.SESS).map((s) => [s, Object.values(s.tokens_by_model ?? {}).length + (s.reported_runs?.length ?? 0)]).sort((a, b) => b[1] - a[1])[0]?.[0]?.id;
  r.expect(!!sid, "the fixture holds no session to open");
  r.expect(!!X.SESS["web-search"], "the extras fixture holds no web-search session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    for (const [name, extras, id] of [["harbor", false, sid], ["web-search", true, "web-search"]]) {
      const tag = size + (dark ? "-dark" : "-light") + " " + name, page = await served(browser, { size, dark, extras });
      await goto(page, { v: "session", id }, extras ? X : D); await page.waitForTimeout(200);
      await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
      await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true); await page.waitForTimeout(150);
      const m = await read(page);
      results[tag] = { tag: m.tag, background: m.background, fonts: m.fonts, groups: m.groups.map((g) => g.model + ": " + g.rows.map((x) => x.kind + " " + (x.tokens || "-") + " " + x.cost).join(" | ")), labels: m.labels.map((l) => l.text).join(", ") };
      r.expect(m.tag === "DIV" && !m.expanded, tag + ": the cost row is " + m.tag + (m.expanded ? " with aria-expanded" : "") + ", not a plain element");
      r.expect(m.background === "rgba(0, 0, 0, 0)", tag + ": the cost row background is " + m.background + ", not transparent");
      r.expect(m.rowSameGrid, tag + ": the cost row does not share the other rows' grid, padding and hairline");
      r.expect(m.valueLeft.every((x) => x === m.valueLeft[0]), tag + ": values do not start at one x: " + JSON.stringify(m.valueLeft));
      r.expect(m.breakdownVisible && !m.hidden && m.groups.length > 0, tag + ": the breakdown is not shown on open");
      r.expect(m.tipIcon, tag + ": the API-equivalent cost tip is not reachable from the sheet");
      r.expect(m.fonts.row < m.fonts.detail && m.fonts.name <= m.fonts.row, tag + ": the breakdown is not quieter than the detail rows: " + JSON.stringify(m.fonts));
      for (const l of m.labels) r.expect(l.lines <= 1, tag + ": the label " + JSON.stringify(l.text) + " wraps onto " + l.lines + " lines");
      r.expect(!m.sideways && m.inView, tag + ": the sheet scrolls sideways or is off screen");
      for (const g of m.groups) {
        r.expect(g.rows.length > 0, tag + ": " + g.model + " lists no rows");
        for (const x of g.rows) {
          r.expect(!((x.tokens === "" || x.tokens === "0") && x.cost === "$0.00"), tag + ": " + g.model + " shows a zero row " + x.kind);
          // The text of the tokens and cost cells ends where their column head's text ends.
          for (const i of [1, 2]) if (x.cells[i].text) r.expect(near(x.cells[i].textRight, g.head[i].textRight), tag + ": " + g.model + " " + x.kind + " column " + i + " text ends at " + x.cells[i].textRight + ", its head at " + g.head[i].textRight);
          // Cells that have text share a baseline; an empty cell has none to compare.
          const based = x.cells.filter((c) => c.text);
          r.expect(based.every((c) => near(c.base, based[0].base)), tag + ": " + g.model + " " + x.kind + " cells do not share a baseline: " + JSON.stringify(based.map((c) => c.base)));
          if (x.tokens) r.expect(/^\d[\d.]*[kM]?$/.test(x.tokens) && /^[\d,]+ tokens$/.test(x.cells[1].tip ?? ""), tag + ": " + g.model + " " + x.kind + " tokens read " + x.tokens + " with tip " + x.cells[1].tip);
        }
      }
      if (name === "web-search") {
        const rows = m.groups.flatMap((g) => g.rows), web = rows.find((x) => x.kind === "Web search");
        r.expect(!!web && web.tokens === "" && web.cost === "$0.04" && web.cells[1].tip === null, tag + ": the web-search row should show an empty tokens cell and $0.04: " + JSON.stringify(web));
        r.expect(rows.some((x) => x.kind === "Input" && x.tokens === "2k") && rows.some((x) => x.kind === "Output" && x.tokens === "3k"), tag + ": the priced token rows are missing: " + JSON.stringify(rows.map((x) => x.kind + " " + x.tokens)));
        r.expect(!rows.some((x) => x.kind === "Cache read" || x.kind.startsWith("Cache write")), tag + ": a zero cache row is shown");
      }
      const shot = "details-" + (extras ? "web-search-" : "") + size + (dark ? "-dark" : "-light");
      await page.screenshot({ path: path.join(OUT, shot + ".png") });
      // The sheet's body may scroll on a phone: a second picture of the bottom, where the notes are.
      await page.evaluate(() => { const vb = document.querySelector("dialog.session-details .vb"); vb.scrollTop = vb.scrollHeight; }); await page.waitForTimeout(100);
      await page.screenshot({ path: path.join(OUT, shot + "-end.png") });
      await page.close();
    }
  }
  // The summary rows and the per-model table read one model. A live update (which adopts a newer model but keeps the session
  // page's top bar) must not leave the ⋯ menu, or Session details opened from a menu that was already open, holding the model the
  // page was drawn from: the table already reads the newest. Measured on a session with no child runs (its table then counts its
  // own tokens alone), with the menu opened after the update and before it.
  const claude = Object.values(D.SESS).find((s) => s.harness === "claude" && !s.stub && !s.parent && !D.H.some((h) => h.from === s.id) && Object.values(s.cost?.by_model ?? {}).some((m) => m.tokens?.cache_read > 0));
  r.expect(!!claude, "the fixture holds no Claude session with cache reads to grow");
  // "172k" and "5.3M" as millions, and whether a text reads the same figure as `m` millions, to the digit it shows.
  const num = (text) => (text.endsWith("M") ? parseFloat(text) : parseFloat(text) / 1000), reads = (text, m) => Math.abs(num(text ?? "0k") - m) <= (text?.endsWith("M") ? 0.051 : 0.0011);
  const readSheet = (page) => page.evaluate(() => {
    const d = document.querySelector("dialog.session-details"), rows = {};
    for (const x of d.querySelectorAll(".detail-row")) rows[x.querySelector(".detail-label").textContent] = x.querySelector(".detail-value").textContent;
    const lines = [...d.querySelectorAll(".cost-line")].map((l) => ({ kind: l.children[0].textContent, tokens: Number((l.children[1].dataset.tip ?? "").replace(/[^\d]/g, "")) || 0 }));
    return { rows, lines, note: d.querySelector(".tokens-own")?.textContent ?? null, head: d.querySelector(".cost-breakdown-head")?.textContent ?? "" };
  });
  if (claude) {
    const modelId = Object.keys(claude.cost.by_model).find((k) => claude.cost.by_model[k].tokens?.cache_read > 0), more = 400000;
    for (const size of ["phone", "desktop"]) {
      for (const order of ["menu after the update", "menu before the update"]) {
        const tag = size + " live growth, " + order, page = await served(browser, { size });
        await goto(page, { v: "session", id: claude.id }, D); await page.waitForTimeout(200);
        if (order === "menu before the update") await page.click("#more-btn");
        let polled = 0;
        // Every poll now gets the model as it would be a moment later: 400k more cache reads, in the summary and in the table.
        await page.route(/\/api\/model\?since=/, async (route) => {
          const url = new URL(route.request().url()), headers = { ...route.request().headers() }; url.search = ""; delete headers["if-none-match"];
          const res = await route.fetch({ url: url.toString(), headers }), m = await res.json(), s = m.sessions[claude.id];
          s.tokens = [s.tokens[0], Math.round((s.tokens[1] + more / 1e6) * 1000) / 1000, s.tokens[2]];
          s.cost.by_model[modelId].tokens.cache_read += more; m.version += "-grown"; polled++;
          await route.fulfill({ response: res, json: m });
        });
        const t0 = Date.now();
        while (polled < 2 && Date.now() - t0 < 8000) await page.waitForTimeout(100);
        r.expect(polled >= 2, tag + ": the page did not poll the model");
        await page.waitForTimeout(400);
        const grown = claude.tokens[1] + more / 1e6;
        if (order === "menu after the update") {
          await page.click("#more-btn");
          const menu = await page.evaluate(() => Object.fromEntries([...document.querySelectorAll(".session-menu dl dt")].map((dt) => [dt.textContent, dt.nextElementSibling.textContent])));
          r.expect(reads(menu["Cache read"], grown) && reads(menu["Output"], claude.tokens[2]) && reads(menu["Input + cache write"], claude.tokens[0]), tag + ": the menu's rows are " + JSON.stringify(menu) + ", not the grown model's");
        }
        await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
        await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true);
        const got = await readSheet(page), sum = (f) => got.lines.filter(f).reduce((n, l) => n + l.tokens, 0);
        const read = sum((l) => l.kind === "Cache read"), written = sum((l) => l.kind.startsWith("Cache write") || l.kind === "Input"), output = sum((l) => l.kind === "Output");
        r.expect(read === claude.cost.by_model[modelId].tokens.cache_read + more, tag + ": the table's cache read is " + read + ", not the grown model's");
        r.expect(reads(got.rows["Cache read"], grown) && reads(got.rows["Cache read"], read / 1e6), tag + ": the summary's Cache read is " + got.rows["Cache read"] + ", the table's is " + read.toLocaleString());
        r.expect(reads(got.rows["Input + cache write"], written / 1e6), tag + ": the summary's Input + cache write is " + got.rows["Input + cache write"] + ", the table's input and cache writes make " + written.toLocaleString());
        r.expect(reads(got.rows["Output"], output / 1e6), tag + ": the summary's Output is " + got.rows["Output"] + ", the table's is " + output.toLocaleString());
        r.expect(got.note === null && !got.head.includes("incl. runs"), tag + ": a session with no runs shows the runs note or heading: " + got.note);
        results[tag] = got.rows;
        r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
        await page.context().close();
      }
    }
  }
  // A session with runs: its table counts them ("incl. runs"), so the summary says it does not.
  const spawned = (s) => D.H.filter((h) => h.kind === "spawn" && h.from === s.id && h.to).length;
  const parent = Object.values(D.SESS).find((s) => spawned(s) === 1) ?? Object.values(D.SESS).find((s) => spawned(s) > 0);
  r.expect(!!parent, "the fixture holds no session with a run");
  if (parent) {
    for (const size of ["phone", "desktop"]) {
      const tag = size + " session with runs", page = await served(browser, { size });
      await goto(page, { v: "session", id: parent.id }, D); await page.waitForTimeout(200);
      await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
      await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true);
      const got = await readSheet(page);
      r.expect(got.head.includes("incl. runs"), tag + ": the table heading does not say it includes runs: " + got.head);
      r.expect(got.note === "This session only; the table below includes its runs.", tag + ": the summary's note reads " + JSON.stringify(got.note));
      for (const label of ["Input + cache write", "Output", "Cache read"]) r.expect(label in got.rows, tag + ": the summary lost its row " + label);
      results[tag] = { note: got.note, head: got.head };
      await page.context().close();
    }
  }
  r.results = results;
  return r.done();
}
