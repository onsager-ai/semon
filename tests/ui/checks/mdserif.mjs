// A table an agent wrote reads in the same face and size as the prose around it: in an assistant message the reading serif
// (docs/design/overhaul.md: one sans for the interface, one serif for what agents write, one mono for code). Opened on the extras
// fixture's harbor session, whose markdown message holds a paragraph, a heading, lists, a blockquote and a table with a code cell.
// Asserts the computed font-family of the paragraph, the heading, the list items, the blockquote, the table's cells and heads is the
// serif; that the table takes the paragraph's size, that its heads are weight 600, its cells tabular-nums (measured: "1111" and
// "0000" are as wide as each other), and its code mono; and that the Session details cost table, which is interface, stays sans.
// The report also holds the width of figures in the table's face with tabular-nums and without, which shows whether the served
// Source Serif 4 has a tnum feature of its own. Screenshots of the message go to out/mdserif/ for the visual pass, at 390 and
// 1280 px, light and dark.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "mdserif");
fs.mkdirSync(OUT, { recursive: true });

const read = (page) => page.evaluate(() => {
  // The three faces as this page resolves them, by a probe that names each variable.
  const face = (v) => { const p = document.createElement("span"); p.style.cssText = "position:fixed;visibility:hidden;font-family:var(" + v + ")"; document.body.append(p); const f = getComputedStyle(p).fontFamily; p.remove(); return f; };
  const faces = { serif: face("--serif"), sans: face("--sans"), mono: face("--mono") };
  const msg = [...document.querySelectorAll(".msg.assistant .body.md")].find((m) => m.querySelector("table") && m.querySelector("code"));
  if (!msg) return { faces, missing: true };
  const cs = (n) => { const s = getComputedStyle(n); return { family: s.fontFamily, size: s.fontSize, weight: s.fontWeight, tabular: s.fontVariantNumeric.includes("tabular-nums") }; };
  const one = (sel) => { const n = msg.querySelector(sel); return n ? cs(n) : null; };
  // Figures in a cell: a probe span of digits in the table's own face, with tabular-nums and with proportional-nums.
  const width = (host, text, numeric) => { const p = document.createElement("span"); p.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap;font-variant-numeric:" + numeric; p.textContent = text; host.append(p); const w = p.getBoundingClientRect().width; p.remove(); return w; };
  const td = msg.querySelector("td");
  const figures = { tabular: [width(td, "1111", "tabular-nums"), width(td, "0000", "tabular-nums")], proportional: [width(td, "1111", "proportional-nums"), width(td, "0000", "proportional-nums")] };
  return {
    faces, p: one("p"), heading: one(".mh"), li: one("li"), quote: one("blockquote"), table: one("table"), th: one("th"), td: one("td"), tdCode: one("td code"),
    figures, serifLoaded: document.fonts.check("16px 'Source Serif 4'"),
    tableWidth: Math.round(msg.querySelector(".tbl").getBoundingClientRect().width), msgWidth: Math.round(msg.getBoundingClientRect().width), sideways: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
});

// The Session details sheet's cost table: its rows are the interface's, so its face is the sans.
const cost = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-details"), n = d?.querySelector(".cost-line, .cost-model-head, .cost-breakdown-head");
  return n ? getComputedStyle(n).fontFamily : null;
});

export default async function mdSerifCheck(browser) {
  const X = await data({ extras: true }), r = reporter("mdserif"), results = {};
  r.expect(!!X.SESS.harbor, "the extras fixture holds no harbor session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), page = await served(browser, { size, dark, extras: true });
    await goto(page, { v: "session", id: "harbor" }, X);
    await page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"]').forEach((x) => x.click()); });
    await page.waitForTimeout(150);
    const m = await read(page);
    if (!r.expect(!m.missing, tag + ": no assistant message with a table and code was found")) { await page.close(); continue; }
    const { serif, sans, mono } = m.faces;
    results[tag] = { faces: m.faces, p: m.p, heading: m.heading, li: m.li, quote: m.quote, table: m.table, th: m.th, td: m.td, tdCode: m.tdCode, figures: m.figures, serifLoaded: m.serifLoaded };
    r.expect(m.serifLoaded, tag + ": Source Serif 4 is not loaded");
    r.expect(serif !== sans && serif !== mono, tag + ": the probe cannot tell the faces apart: " + JSON.stringify(m.faces));
    for (const k of ["p", "heading", "li", "quote", "table", "th", "td"]) {
      r.expect(!!m[k], tag + ": the message holds no " + k);
      if (m[k]) r.expect(m[k].family === serif, tag + ": " + k + " is set in " + m[k].family + ", not the reading serif " + serif);
    }
    r.expect(!!m.tdCode, tag + ": the table holds no code cell");
    if (m.tdCode) r.expect(m.tdCode.family === mono, tag + ": code in a cell is set in " + m.tdCode.family + ", not the mono " + mono);
    r.expect(m.td?.size === m.p?.size && m.th?.size === m.p?.size, tag + ": the table's size is td " + m.td?.size + " th " + m.th?.size + ", the paragraph's " + m.p?.size);
    r.expect(m.th?.weight === "600", tag + ": a table head has weight " + m.th?.weight + ", not 600");
    r.expect(!!m.td?.tabular && !!m.th?.tabular, tag + ": table cells are not tabular-nums");
    r.expect(Math.abs(m.figures.tabular[0] - m.figures.tabular[1]) < 0.05, tag + ": tabular figures are not one width in a cell: " + JSON.stringify(m.figures.tabular));
    r.expect(m.tableWidth <= m.msgWidth + 1 && !m.sideways, tag + ": the table or page sticks out sideways: table " + m.tableWidth + " in message " + m.msgWidth);
    // The message is in view for the picture before the sheet opens over it.
    await page.evaluate(() => { const t = [...document.querySelectorAll(".msg.assistant .body.md")].find((x) => x.querySelector("table")); t.scrollIntoView({ block: "start" }); window.scrollBy(0, -70); });
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(OUT, "mdserif-" + tag + ".png") });
    // The interface's own table stays sans.
    await page.click("#more-btn"); await page.locator(".session-menu [role=menuitem]").filter({ hasText: "Session details" }).click();
    await page.waitForFunction(() => document.querySelector("dialog.session-details")?.open === true); await page.waitForTimeout(150);
    const c = await cost(page); results[tag].costFamily = c;
    r.expect(c === sans, tag + ": the Session details cost table is set in " + c + ", not the interface sans " + sans);
    await page.close();
  }
  r.results = results;
  return r.done();
}
