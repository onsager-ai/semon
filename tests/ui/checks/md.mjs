// Ported from the mockup's md.js: markdown rendering — raw syntax left after rendering (a "before" scan of the raw
// source vs. a "left" scan of the rendered text), overflow at 390px light+dark, link safety, `innerHTML` use, and
// answer cards.
//
// The original ran three passes: 'real' (real logs), 'sample' (test7.html) and 'synthetic' (test7.html plus mkmd.js's
// one message using every markdown construct, including cut-off and injection attempts, loaded from a pre-built
// test-md.html). Ported: 'real' is dropped. 'sample' drives the served fixture. 'synthetic' drives a second served
// instance — the "extras" fixture — whose data is the sample plus mkmd.js's markdown message in harbor, ledger's
// answered two-part question, a 460-entry backlog lane, a long-command Bash step in harbor, an exit-unknown Codex
// call in deps, and an `<img…>`-named XSS lane. Its address is SEMON_EXTRA_BASE/SEMON_EXTRA_TOKEN, opened with a
// small local helper copied from lib.mjs's `served` pattern, since it needs the `innerHTML`-instrumenting init
// script installed before the page's own script runs (lib.mjs's `served` doesn't expose a hook for that). Session
// ids are the sample's in both fixtures, so the session list is read from each server's own data rather than
// hardcoded. Unlike the first port, the synthetic pass is not optional: if SEMON_EXTRA_BASE is unset, the check
// fails outright instead of silently skipping it — the XSS lane and the answered question are the only things in
// either fixture that exercise several of the assertions below.
//
// Assertions:
//  - no page errors.
//  - zero overflow screens, light and dark, sample and synthetic.
//  - no raw markdown syntax left in rendered text (T.left.tableRow/fence/heading/bold all 0): every construct the
//    source actually used is consumed by rendering, not left as literal `**`/`#`/table-row/fence text in the ren-
//    dered blocks. (Text inside <code>/<pre> is excluded from the scan, as in the original, so a legitimately literal
//    `**` inside a code span doesn't count.)
//  - no unsafe or malformed link: badLinks === 0 (every rendered <a> is https/http, opens _blank with rel=noopener)
//    and, in the synthetic pass, the `javascript:` URL in the source is not a live link at all (badLinkIsText) —
//    it must render as inert text, not as an anchor with a dangerous href.
//  - injected === 0: a literal `<script>`/`<b>` in the source text never becomes a real <script>/<b>/<iframe>/<img>
//    element inside a rendered `.md` block — it stays escaped text (the synthetic pass also checks the literal text
//    is present: scriptText).
//  - document-wide, on every screen visited, on both fixtures: exactly one <script> element (the page's own
//    /viewer.js) and zero <img>/<iframe> anywhere in the document (but an attached image's own thumbnail, on
//    /api/attachment), not only inside `.md` — this is the check the
//    extras fixture's XSS-named lane (an `<img…>` session name, rendered outside any `.md` block: in the sidebar,
//    the crumb, the top bar) is for.
//  - htmlWrites === 0: the renderer never uses `innerHTML`, `outerHTML` or `insertAdjacentHTML` (instrumented from
//    the first script on), on either fixture.
//  - the synthetic pass's structural checks: the ordered list starts on 1 with 3 items and a bullet list nested
//    under it, a star bullet renders, the blockquote has its two lines, a rule (<hr>) renders, and ledger's answered
//    question shows the exact answer text both on Home ("You answered: Ship it (Recommended) · Squash the commits")
//    and in its answer card — a positive check that answers render at all and render correctly, since the
//    sample/served model has no answered question anywhere else in this fixture (gaps.json: h18 is not served).
import path from "node:path";
import { ENV, VIEWPORTS, settled, goto, data, reporter, overflow } from "../lib.mjs";

const PAT = { tableRow: /^\|.*\|$/, fence: /```/, heading: /^#{1,6} /, bold: /\*\*/ };

function before(D) {
  const texts = [];
  for (const es of Object.values(D.TX)) for (const e of es) if ((e.k === "a" || e.k === "u") && e.text) texts.push(e.text);
  for (const h of D.H) { if (h.brief) texts.push(h.brief); if (h.result) texts.push(h.result); }
  const c = { tableRow: 0, fence: 0, heading: 0, bold: 0 };
  for (const t of texts) for (const l0 of t.split("\n")) { const l = l0.trim(), nc = l.replace(/`[^`]+`/g, ""); for (const k in PAT) if (PAT[k].test(k === "fence" ? l : nc)) c[k]++; }
  return c;
}

// A small variant of lib.mjs's `served`, parameterized on base/token (for the second, --markdown fixture) and with
// the innerHTML/outerHTML/insertAdjacentHTML instrumentation installed before the page's own script runs.
async function openInstrumented(browser, { base, token, size, dark, path: routePath = "/" }) {
  const ctx = await browser.newContext({ ...VIEWPORTS[size], colorScheme: dark ? "dark" : "light", timezoneId: "UTC", locale: "en-US", reducedMotion: "no-preference" });
  const page = await ctx.newPage();
  page.errors = [];
  page.on("pageerror", (e) => page.errors.push(e.message.split("\n")[0]));
  page.setDefaultTimeout(5000);
  await page.addInitScript(() => {
    window.__html = 0;
    for (const k of ["innerHTML", "outerHTML"]) { const d = Object.getOwnPropertyDescriptor(Element.prototype, k); Object.defineProperty(Element.prototype, k, { get: d.get, set(v) { window.__html++; d.set.call(this, v); }, configurable: true }); }
    const ia = Element.prototype.insertAdjacentHTML; Element.prototype.insertAdjacentHTML = function (...a) { window.__html++; return ia.apply(this, a); };
  });
  await page.clock.setFixedTime(ENV.now);
  await page.route(/.*/, (r) => (r.request().url().startsWith(base + "/") ? r.continue() : r.abort()));
  await page.goto(base + routePath + (routePath.includes("?") ? "&" : "?") + "t=" + token, { waitUntil: "load" });
  await settled(page);
  return page;
}

async function dataFrom(base, token) {
  const get = async (p) => { const r = await fetch(base + p + (p.includes("?") ? "&" : "?") + "t=" + token); if (!r.ok) throw new Error(p + ": " + r.status); return r.json(); };
  const model = await get("/api/model");
  const SESS = model.sessions, H = model.handoffs, TX = {};
  for (const [id, s] of Object.entries(SESS)) s.id = id;
  for (const sid of Object.keys(SESS)) {
    let page = await get("/api/tx?sid=" + encodeURIComponent(sid)), entries = page.entries;
    while (page.from > 0) { page = await get("/api/tx?sid=" + encodeURIComponent(sid) + "&before=" + page.from); entries = page.entries.concat(entries); }
    TX[sid] = entries;
  }
  return { model, SESS, H, TX };
}

async function runPass(page, D, tag, scheme) {
  const over = () => overflow(page);
  // Document-wide element census, on every screen: exactly one <script> (the page's own /viewer.js) and zero
  // <img>/<iframe> anywhere in the document — not scoped to `.md`, so a name or brief that carries an XSS payload
  // (the extras fixture's `<img…>`-named lane) is caught wherever it's rendered, not only inside a markdown block. The one
  // <img> the viewer makes, an attached image's thumbnail or sheet with its src on /api/attachment and
  // no attribute but class, alt, src, width, height, loading and decoding, isn't counted.
  const domCheck = () => page.evaluate(() => ({ scripts: document.querySelectorAll("script").length, imgs: [...document.querySelectorAll("img")].filter((x) => !(x.matches("button.attach > img.attach-img, dialog.image-viewer img.attach-full") && /^\/api\/attachment\?sid=[^&]*&o=\d+&b=\d+&v=[0-9a-f]{16}$/.test(x.getAttribute("src") ?? "") && [...x.attributes].every((a) => ["class", "alt", "src", "width", "height", "loading", "decoding"].includes(a.name)))).length, iframes: document.querySelectorAll("iframe").length }));
  const runScan = () => page.evaluate(() => { const PAT = { tableRow: /^\|.*\|$/, fence: /```/, heading: /^#{1,6} /, bold: /\*\*/ }; const c = { tableRow: 0, fence: 0, heading: 0, bold: 0 }, ex = [];
    for (const blk of document.querySelectorAll(".md p, .md li, .md th, .md td, .md .mh")) { const x = blk.cloneNode(true); x.querySelectorAll("code, pre, ul, ol").forEach((n) => n.remove()); const t = x.textContent.trim();
      for (const k in PAT) if (PAT[k].test(t)) { c[k]++; if (ex.length < 4) ex.push(k + ": " + t.slice(0, 70)); } }
    const a = [...document.querySelectorAll(".md a, .answer a")];
    return { c, ex, md: document.querySelectorAll(".md").length, tables: document.querySelectorAll(".md .tbl").length, codeblocks: document.querySelectorAll(".md .codeblock").length, ol: document.querySelectorAll(".md ol").length, nested: document.querySelectorAll(".md li > ul, .md li > ol").length, headings: document.querySelectorAll(".md .mh").length, quotes: document.querySelectorAll(".md blockquote").length, hr: document.querySelectorAll(".md hr").length, em: document.querySelectorAll(".md em").length, del: document.querySelectorAll(".md del").length,
      links: a.length, badLinks: a.filter((x) => !/^https?:$/.test(x.protocol) || x.target !== "_blank" || !/noopener/.test(x.rel)).map((x) => (x.getAttribute("href") ?? "") + " | " + x.target + " | " + x.rel + " | " + x.closest(".md, .answer")?.className), injected: document.querySelectorAll(".md script, .md b, .md iframe, .md img").length,
      answers: { cards: document.querySelectorAll(".event .answer:not(.none)").length, cardsNoAnswer: document.querySelectorAll(".event .answer.none").length, trace: document.querySelectorAll(".hop .answer:not(.none)").length, traceNoAnswer: document.querySelectorAll(".hop .answer.none").length } }; });
  const T = { tag, scheme, screens: 0, overflowScreens: 0, left: { tableRow: 0, fence: 0, heading: 0, bold: 0 }, leftExamples: [], rendered: {}, links: 0, badLinks: 0, badLinkEx: [], injected: 0, answers: { cards: 0, cardsNoAnswer: 0, trace: 0, traceNoAnswer: 0 }, domViolations: [] };
  const addScan = (rr) => { for (const k in rr.c) T.left[k] += rr.c[k]; T.leftExamples.push(...rr.ex); for (const k of ["tables", "codeblocks", "ol", "nested", "headings", "quotes", "hr", "em", "del"]) T.rendered[k] = (T.rendered[k] ?? 0) + rr[k]; T.links += rr.links; T.badLinks += rr.badLinks.length; if (T.badLinkEx.length < 4) T.badLinkEx.push(...rr.badLinks.slice(0, 2)); T.injected += rr.injected; for (const k in rr.answers) T.answers[k] += rr.answers[k]; };
  // Which screens stick out sideways, and the first elements that do.
  T.overflowWhere = [];
  const overAt = async (where) => { const n = await over(); if (!n) return; T.overflowScreens++; T.overflowWhere.push(where + " (" + n + "): " + JSON.stringify(await page.evaluate(() => { const vw = document.documentElement.clientWidth; return { sw: document.documentElement.scrollWidth, vw, out: [...document.querySelectorAll("body *")].filter((e) => { const r = e.getBoundingClientRect(); return r.width && r.right > vw + 0.5; }).slice(0, 6).map((e) => e.tagName.toLowerCase() + "." + [...e.classList].join(".") + " r=" + Math.round(e.getBoundingClientRect().right) + " " + (e.textContent ?? "").slice(0, 50)) }; }))); await page.screenshot({ path: path.join(ENV.out, "md-overflow-" + T.overflowScreens + ".png"), fullPage: true }); };
  const checkDom = async (where) => { const d = await domCheck(); if (d.scripts !== 1 || d.imgs !== 0 || d.iframes !== 0) T.domViolations.push(where + ": " + JSON.stringify(d)); };
  const sids = Object.keys(D.SESS);
  const traceTurns = [];
  for (const sid of sids) {
    await goto(page, { v: "session", id: sid }, D);
    await page.evaluate(() => { document.querySelectorAll(".event .ev-more:not([hidden])").forEach((x) => x.click()); }); await page.waitForTimeout(60);
    addScan(await runScan()); T.screens++; await overAt("session " + sid); await checkDom("session " + sid);
    (await page.evaluate(() => [...document.querySelectorAll(".turn-end .link")].map((x) => x.closest(".turn").dataset.turn))).forEach((t) => traceTurns.push([sid, t]));
  }
  for (const [sid, t] of traceTurns) { await goto(page, { v: "trace", sid, turn: t }, D); await page.evaluate(() => document.querySelectorAll(".hop .more:not([hidden])").forEach((x) => x.click())); await page.waitForTimeout(40); addScan(await runScan()); T.screens++; await overAt("trace " + sid + "/" + t); await checkDom("trace " + sid + "/" + t); }
  await goto(page, { v: "home" }, D); T.homeAnswers = await page.evaluate(() => [...document.querySelectorAll(".ib .ans")].map((x) => x.textContent.trim().slice(0, 80))); T.screens++; await overAt("home"); await checkDom("home");
  await goto(page, { v: "sessions" }, D); await checkDom("sessions"); await goto(page, { v: "machines" }, D); await checkDom("machines");
  T.leftExamples = T.leftExamples.slice(0, 6); T.htmlWrites = await page.evaluate(() => window.__html); T.errors = page.errors;
  if (tag !== "synthetic") T.before = before(D);

  if (scheme === "light") {
    const shot = async (sel, name) => { for (const sid of sids) { await goto(page, { v: "session", id: sid }, D); const ok = await page.evaluate((sel) => { const x = document.querySelector(sel); if (!x) return false; x.scrollIntoView({ block: "center" }); return true; }, sel); if (ok) { await page.waitForTimeout(200); await page.screenshot({ path: path.join(ENV.out, name) }); return sid; } } return null; };
    if (tag === "synthetic") {
      await goto(page, { v: "session", id: "harbor" }, D);
      await page.evaluate(() => { const x = document.querySelector(".md .codeblock"); x.scrollIntoView({ block: "center" }); }); await page.waitForTimeout(200); await page.screenshot({ path: path.join(ENV.out, "bar-md-code.png") });
      await page.evaluate(() => { document.querySelector(".md .mh").scrollIntoView({ block: "start" }); window.scrollBy(0, -70); }); await page.waitForTimeout(150); await page.screenshot({ path: path.join(ENV.out, "bar-md-synthetic-top.png") });
      T.synthetic = await page.evaluate(() => { const m = document.querySelector(".md .mh").closest(".md"); const cb = [...m.querySelectorAll(".codeblock")]; return { codeblocks: cb.length, firstScrolls: cb[0].scrollWidth > cb[0].clientWidth, unclosedFenceText: cb[1]?.textContent, truncatedTableRows: m.querySelector(".tbl")?.querySelectorAll("tr").length, badLinkIsText: !m.querySelector('a[href^="javascript"]') && m.textContent.includes("a bad link, and"), scriptText: m.textContent.includes("<script>alert(1)</script>"), olStart: m.querySelector("ol")?.children.length, nestedUnderOl: !!m.querySelector("ol li > ul"), star: [...m.querySelectorAll("ul > li")].some((l) => l.textContent === "star bullet"), quote: m.querySelector("blockquote")?.children.length, hr: !!m.querySelector("hr") }; });
      await goto(page, { v: "session", id: "ledger" }, D); T.synthetic.answerCard = await page.evaluate(() => { const a = document.querySelector(".event .answer"); return a ? { head: a.querySelector("b")?.textContent, items: [...a.querySelectorAll("li")].map((li) => li.textContent) } : null; });
      // An answered question is a quiet event: no waiting colour, no fill, the done icon.
      T.synthetic.answeredCard = await page.evaluate(() => {
        const card = document.querySelector(".event:has(.answer)"), svg = card?.querySelector(".ev-head > svg"), iconPath = svg?.querySelector("path")?.getAttribute("d"), probe = document.createElement("span");
        probe.style.cssText = "position:fixed;visibility:hidden;color:var(--wait)"; document.body.append(probe);
        const waitColour = getComputedStyle(probe).color; probe.remove();
        return card ? { waiting: card.classList.contains("waiting"), iconColor: getComputedStyle(svg).color, waitColour, background: getComputedStyle(card).backgroundColor, icon: iconPath } : null;
      });
      const waiting = D.H.find((h) => h.kind === "toyou" && ["question", "decision"].includes(h.ask) && h.status === "wait");
      if (waiting) { await goto(page, { v: "session", id: waiting.from }, D); T.synthetic.waitingCard = await page.evaluate((id) => { const card = [...document.querySelectorAll(".event")].find((x) => x.dataset.h === id), svg = card?.querySelector(".ev-head > svg"), iconPath = svg?.querySelector("path")?.getAttribute("d"), probe = document.createElement("span"); probe.style.cssText = "position:fixed;visibility:hidden;color:var(--wait)"; document.body.append(probe); const waitColour = getComputedStyle(probe).color; probe.remove(); return card ? { waitColour, iconColor: getComputedStyle(svg).color, icon: iconPath, waiting: card.classList.contains("waiting") } : null; }, waiting.id); }
      await goto(page, { v: "home" }, D); await page.evaluate(() => document.querySelector(".ib.quiet")?.scrollIntoView({ block: "center" })); await page.waitForTimeout(150); await page.screenshot({ path: path.join(ENV.out, "bar-answered.png") });
    } else {
      T.shots = { table: await shot(".msg .md .tbl", "bar-md-table.png"), list: await shot(".msg .md ol", "bar-md-list.png") };
    }
  }
  return T;
}

export default async function mdCheck(browser) {
  const D = await data();
  const r = reporter("md");
  const passes = [];

  for (const scheme of ["light", "dark"]) {
    const page = await served_(browser, D, scheme);
    passes.push(await runPass(page, D, "sample", scheme));
    await page.context().close();
  }

  const extraBase = (process.env.SEMON_EXTRA_BASE ?? "").replace(/\/$/, "");
  const extraToken = process.env.SEMON_EXTRA_TOKEN ?? "";
  r.expect(!!extraBase, "SEMON_EXTRA_BASE not set: the synthetic (extras) pass is required and cannot be skipped");
  if (extraBase) {
    const Dextra = await dataFrom(extraBase, extraToken);
    const page = await openInstrumented(browser, { base: extraBase, token: extraToken, size: "phone", dark: false });
    passes.push(await runPass(page, Dextra, "synthetic", "light"));
    await page.context().close();
  }

  r.results.passes = passes;
  const wantAnswer = "You answered: Ship it (Recommended) · Squash the commits";
  const normSpace = (s) => String(s ?? "").replace(/\u2009/g, " ").replace(/\s+/g, " ").trim();
  for (const T of passes) {
    const tag = T.tag + "/" + T.scheme;
    r.expect(T.errors.length === 0, tag + ": page errors: " + T.errors.join(" | "));
    r.expect(T.overflowScreens === 0, tag + ": overflowScreens=" + T.overflowScreens + " " + T.overflowWhere.join(" | "));
    r.expect(T.left.tableRow === 0 && T.left.fence === 0 && T.left.heading === 0 && T.left.bold === 0, tag + ": raw markdown left in rendered text: " + JSON.stringify(T.left) + " " + JSON.stringify(T.leftExamples));
    r.expect(T.badLinks === 0, tag + ": unsafe/malformed links: " + JSON.stringify(T.badLinkEx));
    r.expect(T.injected === 0, tag + ": literal tags became real elements inside a markdown block");
    r.expect(T.domViolations.length === 0, tag + ": unexpected <script>/<img>/<iframe> census on some screens: " + JSON.stringify(T.domViolations.slice(0, 6)));
    r.expect(T.htmlWrites === 0, tag + ": renderer used innerHTML/outerHTML/insertAdjacentHTML");
    if (T.synthetic) {
      r.expect(T.synthetic.badLinkIsText === true, tag + ": javascript: URL rendered as a live link");
      r.expect(T.synthetic.scriptText === true, tag + ": literal <script> text missing");
      r.expect(T.synthetic.olStart === 3, tag + ": ordered list item count=" + T.synthetic.olStart);
      r.expect(T.synthetic.nestedUnderOl === true, tag + ": nested bullet list under the ordered list missing");
      r.expect(T.synthetic.star === true, tag + ": star bullet missing");
      r.expect(T.synthetic.quote === 2, tag + ": blockquote child count=" + T.synthetic.quote);
      r.expect(T.synthetic.hr === true, tag + ": rule (<hr>) missing");
      // A card lists two or more answers, one per question, under "You answered:" (the mockup's answerEl).
      r.expect(JSON.stringify(T.synthetic.answerCard) === JSON.stringify({ head: "You answered:", items: ["Ship it (Recommended)", "Squash the commits"] }), tag + ": ledger's answer card: " + JSON.stringify(T.synthetic.answerCard));
      const doneIcon = "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM8 12.5l2.7 2.7L16 9.8", questionIcon = "M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM9.6 9.4a2.5 2.5 0 1 1 3.4 2.4c-.6.3-1 .8-1 1.5v.4M12 17h.01";
      r.expect(T.synthetic.answeredCard?.waiting === false && T.synthetic.answeredCard?.iconColor !== T.synthetic.answeredCard?.waitColour && T.synthetic.answeredCard?.background === "rgba(0, 0, 0, 0)" && T.synthetic.answeredCard?.icon === doneIcon && T.synthetic.answeredCard?.icon !== questionIcon, tag + ": answered question card kept waiting styling or icon: " + JSON.stringify(T.synthetic.answeredCard));
      r.expect(T.synthetic.waitingCard?.iconColor === T.synthetic.waitingCard?.waitColour && T.synthetic.waitingCard?.waiting === true && T.synthetic.waitingCard?.icon === questionIcon, tag + ": waiting question card lost its waiting colour or question icon: " + JSON.stringify(T.synthetic.waitingCard));
      r.expect(T.homeAnswers.some((answer) => normSpace(answer) === normSpace(wantAnswer)), tag + ": Home does not show the exact answered-question text: " + JSON.stringify(T.homeAnswers));
    }
  }

  return r.done();
}

// A local variant of lib.mjs's `served` for the primary fixture, with the same innerHTML instrumentation as the
// synthetic pass (so 'sample' and 'synthetic' are measured the same way).
async function served_(browser, D, scheme) {
  return openInstrumented(browser, { base: ENV.base, token: ENV.token, size: "phone", dark: scheme === "dark" });
}
