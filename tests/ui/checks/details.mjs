// The ⋯ menu's cost section reads as one quiet list: its "Tokens by model" table, opened, has a row for each kind of token that was
// used or billed (kinds that used nothing and cost nothing are left out), tokens in compact form with the exact figure as the cell's
// tooltip, amounts ending where their column ends, and no label wraps at 390 px. Opened on the session with the most cost detail, and
// on the extras fixture's web-search session (priced searches only in the total), at 390 and 1280 px, light and dark; screenshots go to
// out/details/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "details");
fs.mkdirSync(OUT, { recursive: true });

// Reads the open menu with its tokens table shown. A cell's text is on its own text node (the tooltip's screen-reader text sits beside it).
const read = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-menu"), t = d.querySelector(".tokens");
  const lines = (n) => { const range = document.createRange(); range.selectNodeContents(n); return new Set([...range.getClientRects()].map((q) => Math.round(q.top / 4))).size; }; // the label's own text, in line boxes (a row's height may come from its value)
  const cell = (c) => { const node = c.firstChild, text = node?.nodeType === 3 && node.length ? node : null; if (!text) return { text: "", tip: c.dataset.tip ?? null }; const range = document.createRange(); range.selectNodeContents(text); return { text: text.data, tip: c.dataset.tip ?? null, right: Math.round(range.getBoundingClientRect().right * 10) / 10 }; };
  const rows = [...t.querySelectorAll(".tok-line")].map((l) => { const cells = [...l.children].map(cell); return { kind: cells[0].text, tokens: cells[1].text, cost: cells[2].text, cells }; });
  const body = d.querySelector(".panel-b");
  return {
    shown: !t.hidden && t.getClientRects().length > 0, models: [...t.querySelectorAll(".tok-model")].map((x) => x.textContent), rows,
    labels: [...d.querySelectorAll("dl.kv dt")].map((n) => ({ text: n.textContent, lines: lines(n) })),
    sideways: body.scrollWidth > body.clientWidth + 1 || document.documentElement.scrollWidth > document.documentElement.clientWidth,
    inView: (() => { const r = d.getBoundingClientRect(); return r.left >= 0 && r.right <= innerWidth + 0.5; })(),
  };
});

export default async function detailsCheck(browser) {
  const D = await data(), X = await data({ extras: true }), r = reporter("details"), results = {};
  const sid = Object.values(D.SESS).map((s) => [s, Object.values(s.tokens_by_model ?? {}).length + (s.reported_runs?.length ?? 0)]).sort((a, b) => b[1] - a[1])[0]?.[0]?.id;
  r.expect(!!sid, "the fixture holds no session to open");
  r.expect(!!X.SESS["web-search"], "the extras fixture holds no web-search session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    for (const [name, extras, id] of [["harbor", false, sid], ["web-search", true, "web-search"]]) {
      const tag = size + (dark ? "-dark" : "-light") + " " + name, page = await served(browser, { size, dark, extras });
      await goto(page, { v: "session", id }, extras ? X : D); await page.waitForTimeout(200);
      await page.click("#more-btn"); await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true); await page.waitForTimeout(150);
      await page.click("dialog.session-menu .disclose"); await page.waitForTimeout(150);
      const m = await read(page);
      results[tag] = { models: m.models, rows: m.rows.map((x) => x.kind + " " + (x.tokens || "-") + " " + x.cost) };
      r.expect(m.shown && m.models.length > 0, tag + ": the tokens table is not shown after opening it");
      for (const l of m.labels) r.expect(l.lines <= 1, tag + ": the label " + JSON.stringify(l.text) + " wraps onto " + l.lines + " lines");
      r.expect(!m.sideways && m.inView, tag + ": the menu scrolls sideways or is off screen");
      for (const x of m.rows) {
        r.expect(!((x.tokens === "" || x.tokens === "0") && x.cost === "$0.00"), tag + ": a zero row is shown: " + x.kind);
        if (x.tokens) r.expect(/^\d[\d.]*[kM]?$/.test(x.tokens) && /^[\d,]+ tokens$/.test(x.cells[1].tip ?? ""), tag + ": " + x.kind + " tokens read " + x.tokens + " with tip " + x.cells[1].tip);
        // The amounts share one right edge, and so do the token counts that have text.
      }
      for (const i of [1, 2]) { const edges = m.rows.map((x) => x.cells[i].right).filter((v) => v != null); r.expect(edges.every((v) => Math.abs(v - edges[0]) <= 1), tag + ": column " + i + " does not end at one x: " + JSON.stringify(edges)); }
      if (name === "web-search") r.expect(m.rows.some((x) => x.kind === "Input" && x.tokens === "2k") && m.rows.some((x) => x.kind === "Output" && x.tokens === "3k") && !m.rows.some((x) => x.kind === "Cache read"), tag + ": the priced token rows are wrong: " + JSON.stringify(m.rows.map((x) => x.kind + " " + x.tokens)));
      await page.screenshot({ path: path.join(OUT, "menu-" + (extras ? "web-search-" : "") + size + (dark ? "-dark" : "-light") + ".png") });
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
