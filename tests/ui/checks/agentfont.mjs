// One sans for the interface and for what agents write; one mono for code; no serif anywhere (docs/design/overhaul.md, Type).
// Opened on the extras fixture (its harbor session holds a markdown message with a paragraph, a heading, lists, a blockquote and a
// table with a code cell) at 390 and 1280 px, light and dark. Asserts:
//  - agent text computes to the interface sans, the same family as the top bar's title (the nav): a message's paragraph, heading,
//    list items, blockquote, table, head and cell; a question on Home; a brief on a Trace hop;
//  - the table takes the paragraph's size, its heads are weight 600, its cells tabular-nums (measured: "1111" and "0000" are as
//    wide as each other) and code in a cell is mono;
//  - the Session details cost table stays sans;
//  - no element on Home, the session, its Trace or Analytics computes to Source Serif (or to a bare generic serif), no font
//    face of that family is registered, and no request is made for a Source Serif file.
// The report also holds the widths of figures with tabular-nums and with proportional-nums, which shows whether the sans has a
// tnum feature of its own. Screenshots of a transcript, Home and a brief go to out/agentfont/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import { ENV, served, data, reporter, goto } from "../lib.mjs";

const OUT = path.join(ENV.out, "agentfont");
fs.mkdirSync(OUT, { recursive: true });

// The three faces as this page resolves them, by a probe that names each variable, and the nav's own family.
const faces = (page) => page.evaluate(() => {
  const face = (v) => { const p = document.createElement("span"); p.style.cssText = "position:fixed;visibility:hidden;font-family:var(" + v + ")"; document.body.append(p); const f = getComputedStyle(p).fontFamily; p.remove(); return f; };
  return { sans: face("--sans"), mono: face("--mono"), nav: getComputedStyle(document.querySelector("#topbar .t, #topbar #find")).fontFamily };
});

// The family, size and weight of the first element each selector matches inside `root`.
const styles = (page, sels, root = "body") => page.evaluate(([sels, root]) => {
  const r = document.querySelector(root), out = {};
  for (const [k, sel] of Object.entries(sels)) { const n = r?.querySelector(sel); if (!n) { out[k] = null; continue; } const s = getComputedStyle(n); out[k] = { family: s.fontFamily, size: s.fontSize, weight: s.fontWeight, tabular: s.fontVariantNumeric.includes("tabular-nums") }; }
  return out;
}, [sels, root]);

// Any element in the document set in Source Serif or a bare generic serif, and the registered faces of that family.
const serifs = (page) => page.evaluate(() => {
  const bare = /(^|,\s*)serif$/, hits = [];
  for (const n of document.querySelectorAll("body, body *")) { const f = getComputedStyle(n).fontFamily; if (/source serif/i.test(f) || bare.test(f)) { hits.push(n.tagName.toLowerCase() + (n.className && typeof n.className === "string" ? "." + n.className.trim().split(/\s+/)[0] : "") + ": " + f); if (hits.length >= 5) break; } }
  return { hits, faces: [...document.fonts].filter((f) => /source serif/i.test(f.family)).map((f) => f.family + " " + f.weight) };
});

const messageOf = (page) => page.evaluate(() => {
  const msg = [...document.querySelectorAll(".msg.assistant .body.md")].find((m) => m.querySelector("table") && m.querySelector("code"));
  if (!msg) return null;
  const width = (host, text, numeric) => { const p = document.createElement("span"); p.style.cssText = "position:absolute;visibility:hidden;white-space:nowrap;font-variant-numeric:" + numeric; p.textContent = text; host.append(p); const w = p.getBoundingClientRect().width; p.remove(); return w; };
  const td = msg.querySelector("td");
  msg.dataset.agentfont = "1";
  return {
    figures: { tabular: [width(td, "1111", "tabular-nums"), width(td, "0000", "tabular-nums")], proportional: [width(td, "1111", "proportional-nums"), width(td, "0000", "proportional-nums")] },
    tableWidth: Math.round(msg.querySelector(".tbl").getBoundingClientRect().width), msgWidth: Math.round(msg.getBoundingClientRect().width), sideways: document.documentElement.scrollWidth > document.documentElement.clientWidth,
  };
});

const cost = (page) => page.evaluate(() => {
  const d = document.querySelector("dialog.session-menu"), n = d?.querySelector(".tok-line");
  return n ? getComputedStyle(n).fontFamily : null;
});

const shot = (page, tag, name) => page.screenshot({ path: path.join(OUT, name + "-" + tag + ".png") });

export default async function agentFontCheck(browser) {
  const X = await data({ extras: true }), r = reporter("agentfont"), results = {};
  r.expect(!!X.SESS.harbor, "the extras fixture holds no harbor session");
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), page = await served(browser, { size, dark, extras: true }), requested = [];
    page.on("request", (q) => { if (/source[-_ ]?serif/i.test(q.url())) requested.push(q.url()); });
    const f = await faces(page), res = results[tag] = { faces: f };
    const same = (what, s) => { if (!r.expect(!!s, tag + ": " + what + " was not found")) return; r.expect(s.family === f.sans, tag + ": " + what + " is set in " + s.family + ", not the interface sans " + f.sans); };
    r.expect(f.nav === f.sans, tag + ": the nav is set in " + f.nav + ", not the interface sans " + f.sans);
    r.expect(f.sans !== f.mono, tag + ": the probe cannot tell the sans from the mono: " + JSON.stringify(f));
    const scans = [];

    // Home: a question that went to you.
    await goto(page, { v: "home" }, X);
    const home = await styles(page, { question: ".ib .q" });
    res.home = home; same("a question on Home", home.question);
    scans.push(["home", await serifs(page)]); await shot(page, tag, "home");

    // A transcript: a message with every construct, and the tables in it.
    await goto(page, { v: "session", id: "harbor" }, X);
    await page.evaluate(() => { for (let k = 0; k < 3; k++) document.querySelectorAll('.cw-toggle[aria-expanded="false"]').forEach((x) => x.click()); });
    await page.waitForTimeout(150);
    const m = await messageOf(page);
    if (r.expect(!!m, tag + ": no assistant message with a table and code was found")) {
      const s = await styles(page, { p: "p:not(.mh)", heading: ".mh", li: "li", quote: "blockquote", table: "table", th: "th", td: "td", tdCode: "td code" }, "[data-agentfont]");
      res.message = { ...s, figures: m.figures };
      for (const k of ["p", "heading", "li", "quote", "table", "th", "td"]) same("the message's " + k, s[k]);
      r.expect(!!s.tdCode && s.tdCode.family === f.mono, tag + ": code in a cell is set in " + s.tdCode?.family + ", not the mono " + f.mono);
      r.expect(s.td?.size === s.p?.size && s.th?.size === s.p?.size, tag + ": the table's size is td " + s.td?.size + " th " + s.th?.size + ", the paragraph's " + s.p?.size);
      r.expect(s.th?.weight === "600", tag + ": a table head has weight " + s.th?.weight + ", not 600");
      r.expect(!!s.td?.tabular && !!s.th?.tabular, tag + ": table cells are not tabular-nums");
      r.expect(Math.abs(m.figures.tabular[0] - m.figures.tabular[1]) < 0.05, tag + ": tabular figures are not one width in a cell: " + JSON.stringify(m.figures.tabular));
      r.expect(m.tableWidth <= m.msgWidth + 1 && !m.sideways, tag + ": the table or page sticks out sideways: table " + m.tableWidth + " in message " + m.msgWidth);
      await page.evaluate(() => { document.querySelector("[data-agentfont]").scrollIntoView({ block: "start" }); window.scrollBy(0, -70); });
      await page.waitForTimeout(150); await shot(page, tag, "transcript");
    }
    scans.push(["session", await serifs(page)]);
    // The interface's own table stays sans.
    await page.click("#more-btn");
    await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true); await page.click("dialog.session-menu .disclose"); await page.waitForTimeout(150);
    const c = await cost(page); res.costFamily = c;
    r.expect(c === f.sans, tag + ": the Session details cost table is set in " + c + ", not the interface sans " + f.sans);
    await page.keyboard.press("Escape"); await page.waitForTimeout(100);

    // A brief, on a Trace hop, opened.
    const turn = await page.evaluate(() => document.querySelector(".turn-end .link")?.closest(".turn")?.dataset.turn);
    if (r.expect(!!turn, tag + ": harbor has no trace turn")) {
      await goto(page, { v: "trace", sid: "harbor", turn }, X);
      await page.evaluate(() => document.querySelectorAll(".hop .more:not([hidden])").forEach((x) => x.click())); await page.waitForTimeout(100);
      const hop = await styles(page, { brief: ".hop .brief" });
      res.trace = hop; same("a brief on a Trace hop", hop.brief);
      scans.push(["trace", await serifs(page)]); await shot(page, tag, "brief");
    }
    await goto(page, { v: "analytics" }, X); scans.push(["analytics", await serifs(page)]);

    for (const [where, s] of scans) {
      r.expect(s.hits.length === 0, tag + ": on " + where + " elements are set in a serif: " + JSON.stringify(s.hits));
      r.expect(s.faces.length === 0, tag + ": on " + where + " a Source Serif face is registered: " + JSON.stringify(s.faces));
    }
    r.expect(requested.length === 0, tag + ": a Source Serif file was requested: " + JSON.stringify(requested));
    await page.close();
  }
  r.results = results;
  return r.done();
}
