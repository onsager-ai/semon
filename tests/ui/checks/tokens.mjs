// The visual system's rules for text (docs/design/overhaul.md, principles P3 and P7), on every screen that is drawn, at 390 and
// 1280 px in light and dark:
//   - no text is smaller than 12 px;
//   - every text meets WCAG AA against the colour behind it: 4.5:1, or 3:1 for large text (24 px, or 18.66 px and bold).
// "Faint" is not a text colour, and its ratio is about 2.6:1, so it fails here wherever it is used for words. Text that holds no
// letter or digit (a separator such as "·" or "›") is a rule, not words, and isn't measured. Disabled controls and text that is
// not drawn (hidden, closed drawer, under 2 px) are skipped.
import { served, data, reporter, goto, settled, ENV } from "../lib.mjs";
import path from "node:path";

const audit = (page) => page.evaluate(() => {
  const canvas = document.createElement("canvas").getContext("2d", { willReadFrequently: true });
  const rgba = (css) => { canvas.clearRect(0, 0, 1, 1); canvas.fillStyle = "#000"; canvas.fillStyle = css; canvas.fillRect(0, 0, 1, 1); const [r, g, b, a] = canvas.getImageData(0, 0, 1, 1).data; return [r, g, b, a / 255]; };
  const over = (top, under) => { const a = top[3] + under[3] * (1 - top[3]); return a ? [0, 1, 2].map((i) => (top[i] * top[3] + under[i] * under[3] * (1 - top[3])) / a).concat(a) : [0, 0, 0, 0]; };
  const backdrop = (node) => {
    const layers = []; for (let e = node; e; e = e.parentElement) layers.push(rgba(getComputedStyle(e).backgroundColor));
    let c = rgba(getComputedStyle(document.documentElement).getPropertyValue("--ground") || "#fff"); if (c[3] < 1) c = [255, 255, 255, 1];
    for (const layer of layers.reverse()) c = over(layer, c);
    return c;
  };
  const lum = ([r, g, b]) => { const f = (v) => { v /= 255; return v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4; }; return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b); };
  const ratio = (a, b) => { const x = lum(a), y = lum(b); return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05); };
  const drawn = (e) => {
    const box = e.getBoundingClientRect(); if (box.width < 2 || box.height < 2) return false;
    for (let a = e; a; a = a.parentElement) { const cs = getComputedStyle(a); if (cs.display === "none" || cs.visibility === "hidden") return false; }
    return true;
  };
  const opacityOf = (e) => { let o = 1; for (let a = e; a; a = a.parentElement) o *= parseFloat(getComputedStyle(a).opacity); return o; };
  const seen = new Set(), small = [], low = [];
  let texts = 0;
  const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const text = node.nodeValue.trim(), e = node.parentElement;
    if (!text || !/[\p{L}\p{N}]/u.test(text) || !e || e.closest("svg, script, style, title, [disabled], [aria-disabled='true']") || !drawn(e)) continue;
    const cs = getComputedStyle(e), size = parseFloat(cs.fontSize), weight = Number(cs.fontWeight);
    texts++;
    const name = e.tagName.toLowerCase() + (e.className && typeof e.className === "string" ? "." + e.className.trim().split(/\s+/).join(".") : "");
    const key = name + "|" + text.slice(0, 24);
    if (seen.has(key)) continue; seen.add(key);
    if (size < 12) small.push({ el: name, text: text.slice(0, 40), size });
    const bg = backdrop(e), fg0 = rgba(cs.color), fg = over([fg0[0], fg0[1], fg0[2], fg0[3] * opacityOf(e)], bg);
    const large = size >= 24 || (size >= 18.66 && weight >= 700), need = large ? 3 : 4.5, got = Math.round(ratio(fg, bg) * 100) / 100;
    if (got < need) low.push({ el: name, text: text.slice(0, 40), ratio: got, need, size });
  }
  return { texts, small: small.slice(0, 12), smallCount: small.length, low: low.slice(0, 12), lowCount: low.length };
});

const tokenColors = (page) => page.evaluate(() => {
  const probe = document.createElement("span");
  probe.style.position = "fixed"; probe.style.visibility = "hidden";
  document.body.append(probe);
  probe.style.color = "var(--ink)"; const ink = getComputedStyle(probe).color;
  probe.style.color = "var(--accent)"; const accent = getComputedStyle(probe).color;
  probe.remove();
  return { ink, accent };
});

const styledText = (page, selector) => page.evaluate((sel) => {
  const el = document.querySelector(sel); if (!el) return null;
  const style = getComputedStyle(el);
  return { color: style.color, weight: Number(style.fontWeight), text: el.textContent.trim() };
}, selector);

const legacyHues = (page) => page.evaluate(() => {
  const old = new Set(["rgb(59, 77, 191)", "rgb(152, 164, 243)"]), hits = [];
  for (const el of document.querySelectorAll("*")) {
    const style = getComputedStyle(el), values = [style.color, style.backgroundColor, style.borderTopColor, style.borderRightColor, style.borderBottomColor, style.borderLeftColor];
    if (values.some((value) => old.has(value))) hits.push({ tag: el.tagName.toLowerCase(), className: typeof el.className === "string" ? el.className : "", values: values.filter((value) => old.has(value)) });
    if (hits.length === 8) break;
  }
  return hits;
});

export default async function tokensCheck(browser) {
  const D = await data(), r = reporter("tokens"), results = {};
  const harbor = D.SESS.harbor ? "harbor" : Object.keys(D.SESS)[0];
  const parent = Object.values(D.SESS).find((s) => Object.values(D.SESS).filter((c) => c.parent === s.id).length >= 5)?.id ?? harbor;
  const originFor = (sid) => D.H.find((h) => (h.kind === "spawn" || h.kind === "relay") && h.to === sid && h.from !== sid &&
    (h.kind === "spawn" || D.SESS[sid]?.kind === "Relayed" || !D.SESS[sid]?.lane));
  const introHandoff = D.H.find((h) => h.kind === "spawn" && h.to && h.brief && D.SESS[h.from] && D.SESS[h.to]);
  const headerSession = Object.keys(D.TX).find((sid) => D.TX[sid].some((entry) => {
    const h = entry.k === "h" ? D.H.find((item) => item.id === entry.id) : null;
    return h && h.to === sid && (h.kind === "spawn" || h.kind === "relay") && h.id !== originFor(sid)?.id;
  }));
  r.expect(!!headerSession, "the fixture needs a received turn header with a .turn-h .from control");
  r.expect(!!introHandoff, "the fixture needs a received spawn brief for the Show more card");
  const turn = D.turns.find((t) => t.sent.length);
  const screens = [
    ["home", { v: "home" }], ["sessions", { v: "sessions" }], ["analytics", { v: "analytics" }], ["machines", { v: "machines" }],
    ["session-" + harbor, { v: "session", id: harbor }], ["session-" + parent, { v: "session", id: parent }],
    ...(headerSession ? [["inkaccent-header", { v: "session", id: headerSession }]] : []),
    ...(introHandoff ? [["inkaccent-brief", { v: "session", id: introHandoff.to }]] : []),
    ...(turn ? [["trace", { v: "trace", sid: turn.sid, turn: turn.id }]] : []),
  ];
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const width = size === "phone" ? 390 : 1280, scheme = dark ? "dark" : "light";
    const tag = size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await served(browser, { size, dark });
    if (introHandoff) {
      await page.route("**/api/model*", async (route) => {
        const response = await route.fetch(); let model;
        try { model = await response.json(); } catch { return route.fulfill({ response }); }
        const h = model.handoffs?.find((item) => item.id === introHandoff.id);
        if (h) {
          const continuation = "Keep the handoff scoped to this request, preserve its context, and report what changed before returning the result.";
          h.brief = [h.brief, ...Array(8).fill(continuation)].filter(Boolean).join("\n\n");
        }
        return route.fulfill({ status: response.status(), contentType: "application/json", body: JSON.stringify(model) });
      });
      await page.reload({ waitUntil: "load" });
      await settled(page);
    }
    const tokens = await tokenColors(page);
    r.expect(tokens.accent === tokens.ink, tag + ": --accent does not compute to --ink: " + JSON.stringify(tokens));
    const judge = (where, a) => {
      rec[where] = { texts: a.texts, small: a.smallCount, low: a.lowCount };
      r.expect(a.texts > 5, tag + " " + where + ": only " + a.texts + " texts were measured");
      r.expect(a.smallCount === 0, tag + " " + where + ": " + a.smallCount + " texts are under 12px: " + JSON.stringify(a.small));
      r.expect(a.lowCount === 0, tag + " " + where + ": " + a.lowCount + " texts are under AA contrast: " + JSON.stringify(a.low));
    };
    for (const [name, route] of screens) {
      await goto(page, route, D);
      await page.waitForTimeout(150);
      judge(name, await audit(page));
      if (route.v === "home" || route.v === "analytics" || route.v === "session") {
        const hits = await legacyHues(page);
        r.expect(hits.length === 0, tag + " " + name + ": legacy accent colors remain in computed color, background, or border: " + JSON.stringify(hits));
      }
      if (name === "home") {
        rec.homeUnreadDots = await page.locator(".ib .unread-dot").count();
        await page.screenshot({ path: path.join(ENV.out, "inkaccent-" + width + "-" + scheme + "-home.png"), fullPage: true });
      }
      if (name === "inkaccent-header") {
        const from = await styledText(page, ".turn-h .from");
        r.expect(!!from && from.color === tokens.ink && from.weight >= 500, tag + ": .turn-h .from is missing, not ink, or below weight 500: " + JSON.stringify(from));
      }
      if (name === "inkaccent-brief") {
        const facts = await page.evaluate(() => {
          const crumb = document.querySelector(".topbar .crumb"), more = document.querySelector(".child-intro .more"), open = document.querySelector(".child-intro .intro-open");
          const drawn = (el) => !!el && !el.hidden && el.getClientRects().length > 0;
          return { crumb: !!crumb, more: drawn(more), moreText: more?.textContent.trim() ?? null, open: drawn(open), openText: open?.textContent.trim() ?? null };
        });
        r.expect(facts.crumb && facts.more && facts.moreText === "Show more" && facts.open && facts.openText.startsWith("Open in "), tag + ": received brief card must show Show more, Open in, and a breadcrumb: " + JSON.stringify(facts));
        const crumb = await styledText(page, ".topbar .crumb"), more = await styledText(page, ".child-intro .more");
        r.expect(crumb?.color === tokens.ink && crumb.weight >= 500, tag + ": breadcrumb is not ink at weight 500: " + JSON.stringify(crumb));
        r.expect(more?.color === tokens.ink && more.weight >= 500, tag + ": handoff Show more is not ink at weight 500: " + JSON.stringify(more));
        rec.briefSession = introHandoff.to;
        await page.screenshot({ path: path.join(ENV.out, "inkaccent-" + width + "-" + scheme + "-session.png"), fullPage: true });
        if (size === "desktop") {
          await page.locator(".child-intro .more").hover();
          const hover = await page.locator(".child-intro .more").evaluate((el) => { const style = getComputedStyle(el); return { line: style.textDecorationLine, offset: style.textUnderlineOffset }; });
          r.expect(hover.line.includes("underline") && hover.offset === "3px", tag + ": Show more is not underlined on hover: " + JSON.stringify(hover));
        }
      }
    }
    // The phone's navigation drawer is closed everywhere else; open it on Home. On desktop the sidebar is drawn beside every screen.
    if (size === "phone") { await goto(page, { v: "home" }, D); await page.click("#lead-btn"); await page.waitForTimeout(320); judge("drawer", await audit(page)); }
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  r.results = results;
  return r.done();
}
