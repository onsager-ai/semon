// The harness marks: wherever the viewer names a harness it draws the harness's official mark (an <img> of /harness/*.svg in a sized
// container, built by harnessIcon in viewer.js), for a Claude, a Codex and an OpenCode session, at 390 and 1280 px, in light and dark.
// The viewer reads no OpenCode logs, so the fixture's most recent Claude sessions are answered as OpenCode ones by /api/model here.
//   - the right file is shown for the theme, by prefers-color-scheme and by the explicit data-theme override: Claude's one file in
//     both, Codex's codex-black.svg on light and codex.svg on dark, OpenCode's light file on light and its dark file on dark;
//   - the mark is only ever an <img alt=""> in its container, at the container's size, with no filter or transform on the artwork;
//   - a mark that stands alone (the sidebar) has the accessible name and the tip on its container; one beside text is aria-hidden;
//   - the state dot is still there beside the mark, and the mark is never inside it;
//   - the artwork served is byte for byte the file in assets/harnesses (SHA256SUMS);
//   - the trademark note is in the session menu, once.
// Not covered, because other work is rewriting them: the top bar's model label and kind badge, and the four filter controls.
// Screenshots go to out/hicons/ for the visual pass.
import fs from "node:fs";
import path from "node:path";
import crypto from "node:crypto";
import { fileURLToPath } from "node:url";
import { ENV, served, data, reporter, goto, settled } from "../lib.mjs";

const here = path.dirname(fileURLToPath(import.meta.url));
const ASSETS = path.resolve(here, "../../../assets/harnesses");
const OUT = path.join(ENV.out, "hicons");
fs.mkdirSync(OUT, { recursive: true });

// The file each harness shows on a light and on a dark theme, and its names. Written out here, not read from the viewer.
const FILES = { claude: ["claude-code.svg", "claude-code.svg"], codex: ["codex-black.svg", "codex.svg"], opencode: ["opencode-light.svg", "opencode-dark.svg"] };
const NAME = { claude: "Claude Code", codex: "Codex", opencode: "OpenCode" };
const SHORT = { claude: "Claude", codex: "Codex", opencode: "OpenCode" };
const NOTICE = "Third-party trademarks are the property of their respective owners. Semon is not affiliated with or endorsed by these companies.";

// What each place holds: where its marks are, whether one stands alone, and the size of its container. `text` names what the
// text beside the mark must read, for the places where that is the harness's own name.
const PLACES = {
  sidebar: { selector: "#lanes .srow .hicon", alone: true, size: 14 },
  "sessions-row": { selector: ".page .nrow .session-row-meta > .hicon", alone: true, size: 14 },
  "sessions-group": { selector: ".page .sec-h > .hicon", size: 14, text: (h) => NAME[h] },
  "home-working": { selector: ".page .nrow .ag > .hicon", size: 14, text: (h) => NAME[h] },
  "home-inbox": { selector: ".page .ib .ctx > span > .hicon", size: 14, text: (h) => NAME[h] },
  "analytics-top": { selector: ".page .analytics-session .hlabel > .hicon", size: 14, text: (h) => SHORT[h] },
  "analytics-legend": { selector: ".page .analytics-legend .hicon", size: 14, text: (h) => SHORT[h] },
  details: { selector: "dialog.session-menu dl.kv dd .hicon", size: 16, text: (h) => NAME[h] },
  trace: { selector: ".page .chip-h > .hicon", size: 14 },
  "turn-header": { selector: ".page .turn-h > .hicon", size: 16 },
  "child-kind": { selector: ".page .child-card .cc-meta > .hicon", size: 14 },
};

// Every mark matching `selector` that is drawn (a collapsed sidebar group holds marks that are not), as the page shows it.
const read = (page, selector) => page.evaluate((sel) => [...document.querySelectorAll(sel)].filter((box) => box.getClientRects().length > 0).map((box) => {
  const imgs = [...box.querySelectorAll(":scope > img")], shown = imgs.filter((i) => getComputedStyle(i).display !== "none");
  const b = box.getBoundingClientRect(), parent = box.parentElement;
  const beside = [...parent.childNodes].filter((n) => n !== box).map((n) => (n.nodeType === 1 && n.matches(".sr-only") ? "" : n.textContent)).join("").trim();
  const style = (i) => getComputedStyle(i);
  return {
    harness: box.dataset.harness, imgs: imgs.length, shown: shown.map((i) => i.getAttribute("src")), alts: imgs.map((i) => i.getAttribute("alt")),
    loaded: shown.every((i) => i.complete && i.naturalWidth > 0), untouched: shown.every((i) => style(i).filter === "none" && style(i).objectFit === "contain" && style(i).transform === "none" && style(i).opacity === "1"),
    inside: shown.every((i) => { const r = i.getBoundingClientRect(); return r.left >= b.left - 0.5 && r.right <= b.right + 0.5 && r.top >= b.top - 0.5 && r.bottom <= b.bottom + 0.5 && r.width > 0; }),
    role: box.getAttribute("role"), label: box.getAttribute("aria-label"), tip: box.dataset.tip ?? null, hidden: box.getAttribute("aria-hidden"),
    width: Math.round(b.width * 10) / 10, height: Math.round(b.height * 10) / 10, drawn: b.width > 0 && b.height > 0,
    insideDot: !!box.closest(".dot"), beside, parentTag: parent.tagName,
  };
}), selector);

// Waits for every mark that is shown to have loaded its file (the one hidden by the theme is loaded only when a theme shows it).
const loaded = (page) => page.waitForFunction(() => [...document.querySelectorAll(".hicon img")].filter((i) => getComputedStyle(i).display !== "none").every((i) => i.complete && i.naturalWidth > 0), null, { timeout: 5000 }).catch(() => {});

export default async function hiconsCheck(browser) {
  const D = await data(), r = reporter("hicons"), results = {};

  // The artwork served is the artwork committed: byte for byte, by its SHA-256.
  const sums = fs.readFileSync(path.join(ASSETS, "SHA256SUMS"), "utf8").trim().split("\n").map((line) => line.trim().split(/\s+/));
  r.expect(sums.length === 5, "SHA256SUMS lists " + sums.length + " files, not 5");
  const probe = await served(browser, { size: "desktop" });
  results.served = {};
  for (const [want, file] of sums) {
    const response = await probe.request.get(ENV.base + "/harness/" + file), body = Buffer.from(await response.body());
    const got = crypto.createHash("sha256").update(body).digest("hex");
    results.served[file] = got === want;
    r.expect(response.status() === 200 && got === want, "/harness/" + file + " is served as " + got + " (status " + response.status() + "), not " + want);
    r.expect(got === crypto.createHash("sha256").update(fs.readFileSync(path.join(ASSETS, file))).digest("hex"), "/harness/" + file + " differs from assets/harnesses/" + file);
  }
  await probe.context().close();

  // The sessions whose harness the fixture is made to report as OpenCode: the most recent Claude ones at the top of the sidebar.
  const lanes = Object.values(D.SESS).filter((s) => s.lane).sort((a, b) => b.last - a.last);
  const opencode = lanes.filter((s) => s.harness === "claude").slice(0, 2).map((s) => s.id);
  const claude = lanes.find((s) => s.harness === "claude" && !opencode.includes(s.id)), codex = lanes.find((s) => s.harness === "codex");
  r.expect(opencode.length === 2 && !!claude && !!codex, "the fixture must hold three top-level Claude sessions and a Codex one");
  const relay = [...new Set(D.H.filter((h) => h.kind === "relay" && D.SESS[h.to] && D.SESS[h.from]).map((h) => h.to))].slice(0, 6);
  const spawner = [...new Set(D.H.filter((h) => h.kind === "spawn" && D.SESS[h.from] && D.SESS[h.to]).map((h) => h.from))].slice(0, 6);
  const turn = D.turns.find((t) => t.sent.length);
  r.expect(relay.length > 0 && spawner.length > 0 && !!turn, "the fixture must hold a relay, a spawn and a turn that sent something");

  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = size + (dark ? "-dark" : "-light"), rec = (results[tag] = {}), page = await served(browser, { size, dark });
    // Reports the fixture's most recent Claude sessions as OpenCode ones. Polls (?since=) are answered as they are.
    await page.route("**/api/model*", async (route) => {
      const response = await route.fetch();
      if (response.status() !== 200 || new URL(route.request().url()).searchParams.has("since")) return route.fulfill({ response });
      const body = await response.json();
      for (const id of opencode) if (body.sessions?.[id]) body.sessions[id].harness = "opencode";
      await route.fulfill({ response, json: body });
    });
    await page.reload({ waitUntil: "load" });
    await settled(page);
    const shot = (name) => page.screenshot({ path: path.join(OUT, name + "-" + tag + ".png") });

    // Judges every mark of a place: the file for the theme (`isDark`), the container, the accessible name, the dot.
    const judge = (place, marks, { isDark = dark, need = 1 } = {}) => {
      const spec = PLACES[place], seen = new Set();
      rec[place] = { count: marks.length, harnesses: [...new Set(marks.map((m) => m.harness))].sort() };
      r.expect(marks.length >= need, tag + " " + place + ": " + marks.length + " marks, expected at least " + need);
      for (const m of marks) {
        const at = tag + " " + place + " (" + m.harness + ")";
        r.expect(FILES[m.harness] != null, at + ": unknown harness " + m.harness);
        if (!FILES[m.harness]) continue;
        seen.add(m.harness);
        const [light, darkFile] = FILES[m.harness], same = light === darkFile;
        r.expect(m.imgs === (same ? 1 : 2), at + ": holds " + m.imgs + " images, expected " + (same ? 1 : 2));
        r.expect(m.shown.length === 1 && m.shown[0] === "/harness/" + (isDark ? darkFile : light), at + ": shows " + JSON.stringify(m.shown) + " on " + (isDark ? "dark" : "light") + ", expected /harness/" + (isDark ? darkFile : light));
        r.expect(m.alts.every((a) => a === ""), at + ": an image has alt " + JSON.stringify(m.alts));
        r.expect(m.loaded && m.untouched && m.inside, at + ": the artwork is not drawn as it is: " + JSON.stringify({ loaded: m.loaded, untouched: m.untouched, inside: m.inside }));
        r.expect(m.drawn && m.width === spec.size && m.height === spec.size, at + ": the container is " + m.width + "×" + m.height + ", not " + spec.size + "×" + spec.size);
        r.expect(!m.insideDot, at + ": the mark is inside a state dot");
        if (spec.alone) r.expect(m.role === "img" && m.label === NAME[m.harness] && m.tip === NAME[m.harness] && m.hidden == null, at + ": a lone mark needs role=img, aria-label and tip " + JSON.stringify(NAME[m.harness]) + ": " + JSON.stringify({ role: m.role, label: m.label, tip: m.tip, hidden: m.hidden }));
        else {
          r.expect(m.hidden === "true" && m.role == null && m.label == null && m.tip == null, at + ": a mark beside text is decorative (aria-hidden, no role, name or tip): " + JSON.stringify({ role: m.role, label: m.label, tip: m.tip, hidden: m.hidden }));
          r.expect(/[\p{L}\p{N}]/u.test(m.beside), at + ": no text beside the mark (" + JSON.stringify(m.beside) + ")");
          if (spec.text) r.expect(m.beside.startsWith(spec.text(m.harness)), at + ": the text beside it reads " + JSON.stringify(m.beside) + ", not " + JSON.stringify(spec.text(m.harness)));
        }
      }
      return seen;
    };

    // ---- The sidebar: a lone mark after each name, beside the state dot ----
    if (size === "phone") { await page.click("#lead-btn"); await page.waitForTimeout(320); } // the drawer is the sidebar on a phone
    await loaded(page);
    const side = await read(page, PLACES.sidebar.selector);
    const seen = judge("sidebar", side, { need: 3 });
    r.expect(["claude", "codex", "opencode"].every((h) => seen.has(h)), tag + " sidebar: marks for " + [...seen].join(", ") + ", expected claude, codex and opencode");
    const dots = await page.evaluate(() => ({ rows: document.querySelectorAll("#lanes .srow").length, dots: document.querySelectorAll("#lanes .srow-main > .dot").length, marks: document.querySelectorAll("#lanes .session-row-meta > .hicon").length, dotFirst: [...document.querySelectorAll("#lanes .srow-main")].every((m) => m.firstElementChild?.classList.contains("dot")) }));
    rec.dots = dots;
    r.expect(dots.rows > 0 && dots.dots === dots.rows && dots.marks === dots.rows && dots.dotFirst, tag + " sidebar: every row keeps its state dot first and has one mark: " + JSON.stringify(dots));
    // The explicit theme override wins over the browser's scheme, and removing it hands the choice back.
    for (const [attr, isDark] of [["light", false], ["dark", true], [null, dark]]) {
      await page.evaluate((v) => (v ? document.documentElement.setAttribute("data-theme", v) : document.documentElement.removeAttribute("data-theme")), attr);
      await loaded(page);
      judge("sidebar", await read(page, PLACES.sidebar.selector), { isDark, need: 3 });
      rec["override-" + attr] = "checked";
    }
    await shot("sidebar");
    if (size === "phone") { await page.click("#drawer-close"); await page.waitForTimeout(320); }

    // ---- Sessions: rows, and the groups when grouped by harness ----
    await goto(page, { v: "sessions" }, D); await loaded(page);
    judge("sessions-row", await read(page, PLACES["sessions-row"].selector), { need: 3 });
    await page.click('.page .groupby button[data-g="harness"]'); await page.waitForTimeout(150); await loaded(page);
    const groups = await read(page, PLACES["sessions-group"].selector);
    const grouped = judge("sessions-group", groups, { need: 3 });
    r.expect(["claude", "codex", "opencode"].every((h) => grouped.has(h)), tag + " sessions-group: headings for " + [...grouped].join(", "));
    await shot("sessions-by-harness");
    await page.click('.page .groupby button[data-g="recent"]'); await page.waitForTimeout(100);
    await shot("sessions");

    // ---- Home: a working row names its harness, and an inbox item's context line ----
    await goto(page, { v: "home" }, D); await loaded(page);
    judge("home-working", await read(page, PLACES["home-working"].selector));
    judge("home-inbox", await read(page, PLACES["home-inbox"].selector));
    await shot("home");

    // ---- Analytics: the harness label in the ranked lists, and the chart legends ----
    await goto(page, { v: "analytics" }, D); await loaded(page);
    const tops = await read(page, PLACES["analytics-top"].selector);
    judge("analytics-top", tops);
    const labels = await page.evaluate(() => [...document.querySelectorAll(".page .analytics-session .hlabel")].map((l) => ({ mark: l.querySelector(".hicon")?.dataset.harness ?? null, name: l.querySelector(".hname")?.textContent ?? null, only: [...l.children].map((c) => c.className.split(" ")[0]).join(",") })));
    rec.labels = labels.slice(0, 6);
    r.expect(labels.length > 0 && labels.every((l) => l.mark && SHORT[l.mark] === l.name && l.only === "hicon,hname"), tag + " analytics-top: a label and its mark disagree: " + JSON.stringify(labels.filter((l) => !(l.mark && SHORT[l.mark] === l.name)).slice(0, 3)));
    const legend = await read(page, PLACES["analytics-legend"].selector);
    judge("analytics-legend", legend, { need: 2 });
    r.expect(["claude", "codex"].every((h) => legend.some((m) => m.harness === h)), tag + " analytics-legend: not both Claude and Codex");
    await page.locator(".analytics-legend").first().scrollIntoViewIfNeeded(); await page.waitForTimeout(100);
    await shot("analytics");
    await page.locator(".analytics-session").first().scrollIntoViewIfNeeded(); await page.waitForTimeout(100);
    await shot("analytics-list");

    // ---- The session menu: the Harness row, and the one trademark note ----
    for (const [name, sid, harness] of [["claude", claude?.id, "claude"], ["codex", codex?.id, "codex"], ["opencode", opencode[0], "opencode"]]) {
      if (!sid) continue;
      await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(150);
      await page.click("#more-btn");
      await page.waitForFunction(() => document.querySelector("dialog.session-menu")?.open === true); await loaded(page); await page.waitForTimeout(100);
      const marks = await read(page, PLACES.details.selector);
      judge("details", marks, { need: 1 });
      r.expect(marks.length === 1 && marks[0].harness === harness, tag + " details (" + name + "): the Harness row's mark is " + JSON.stringify(marks.map((m) => m.harness)));
      const note = await page.evaluate(() => ({ own: [...document.querySelectorAll("dialog.session-menu .third-party")].map((n) => n.textContent), total: document.body.innerText.split("Third-party trademarks").length - 1, size: parseFloat(getComputedStyle(document.querySelector("dialog.session-menu .third-party")).fontSize) }));
      rec["note-" + name] = note.total;
      r.expect(note.own.length === 1 && note.own[0] === NOTICE && note.total === 1, tag + " details (" + name + "): the trademark note is " + JSON.stringify(note) + ", expected exactly once");
      r.expect(note.size >= 12, tag + " details (" + name + "): the note is " + note.size + "px");
      if (name === "opencode") { await shot("details"); await page.evaluate(() => { const vb = document.querySelector("dialog.session-menu .panel-b"); vb.scrollTop = vb.scrollHeight; }); await page.waitForTimeout(100); await shot("details-end"); }
      await page.evaluate(() => document.querySelector("dialog.session-menu")?.close()); await page.waitForTimeout(300);
    }
    r.expect(await page.evaluate(() => document.body.innerText.split("Third-party trademarks").length - 1) === 0, tag + ": the trademark note is shown outside the session menu");

    // ---- A trace's session chip ----
    if (turn) {
      await goto(page, { v: "trace", sid: turn.sid, turn: turn.id }, D); await loaded(page);
      judge("trace", await read(page, PLACES.trace.selector));
      await shot("trace");
    }

    // ---- A session that received a relay: its header names the sender's harness ----
    let relayed = false;
    for (const sid of relay) {
      await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(150); await loaded(page);
      const marks = await read(page, PLACES["turn-header"].selector);
      if (!marks.length) continue;
      judge("turn-header", marks); relayed = true; await shot("relay-header"); break;
    }
    r.expect(relayed, tag + " turn-header: no session that received a relay shows a turn header with a mark");

    // ---- A session that handed work off: the child card's kind badge ----
    let carded = false;
    for (const sid of spawner) {
      await goto(page, { v: "session", id: sid }, D); await page.waitForTimeout(150); await loaded(page);
      const marks = await read(page, PLACES["child-kind"].selector);
      if (!marks.length) continue;
      judge("child-kind", marks);
      const badge = await page.evaluate(() => [...document.querySelectorAll(".page .child-card .cc-meta")].every((m) => { const mk = m.querySelector(":scope > .hicon")?.getBoundingClientRect(), b = m.getBoundingClientRect(); return !!mk && mk.top >= b.top - 2 && mk.bottom <= b.top + 24; }));
      r.expect(badge, tag + " child-kind: a card's mark is not on the first line of its kind line");
      carded = true; await shot("child-card"); break;
    }
    r.expect(carded, tag + " child-kind: no session that handed work off shows a child card with a mark");

    r.expect(await page.evaluate(() => document.documentElement.scrollWidth <= document.documentElement.clientWidth), tag + ": the page scrolls sideways");
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join("; "));
    await page.context().close();
  }
  r.results = results;
  return r.done();
}
