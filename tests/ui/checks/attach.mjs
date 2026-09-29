// Images a prompt attaches, on the extras fixture's `attach` lane (fixture.mjs), at 390 and 1280 px, light and dark:
//   - /api/tx names each image (its line's offset and block, type, size, content version, width and height) and holds none
//     of its bytes, and no "[Image #1]" placeholder or "[image]" text; /api/attachment serves one as image/png with nosniff,
//     an immutable private cache and a CSP of its own, and refuses a block that isn't an image, another session's offset, a
//     version that isn't the content's and the masked image;
//   - the transcript shows a thumbnail for each image (loaded, at most 200×160, its shape kept, alt "Attached image N (PNG,
//     …)") in your messages, the queued one, the image-only one and a prompt that isn't yours, and a quiet "Image not
//     available" chip for the masked one, with no "[image]" text anywhere and nothing past the screen's edge; each
//     thumbnail's box is the same before its image arrives (held back) and after, so nothing moves as they load;
//   - tapping a thumbnail opens the image whole in the viewer sheet (loaded, inside the screen); Escape closes it and focus
//     returns to the thumbnail; on a phone the back gesture closes it too;
//   - the embedding API fixture serves the image with the same headers;
//   - no page errors.
// Screenshots: attach-<390|1280>-<light|dark>.png (the thumbnails), attach-chip-….png (the image-only message and the chip)
// and attach-open-….png (the sheet).
import path from "node:path";
import { ENV, served, settled, reporter, overflow } from "../lib.mjs";

const SID = "attach";

// The transcript's images and chips, and what the page says in text.
const census = (page) => page.evaluate(() => {
  const tx = document.querySelector("#page section[aria-label='Transcript']") ?? document.querySelector("#page");
  const thumbs = [...tx.querySelectorAll("button.attach > img.attach-img")].map((img) => {
    const box = img.getBoundingClientRect();
    return { alt: img.alt, src: img.getAttribute("src"), loaded: img.complete && img.naturalWidth > 0, w: box.width, h: box.height, nw: img.naturalWidth, nh: img.naturalHeight, inUser: !!img.closest(".msg.user") };
  });
  return { thumbs, chips: [...tx.querySelectorAll(".attach-na")].map((c) => ({ text: c.textContent, inUser: !!c.closest(".msg.user") })), text: tx.textContent, bubbles: tx.querySelectorAll(".msg.user").length };
});

// Scrolls each thumbnail into view until it has loaded (they load lazily), then back to the first.
async function loadAll(page) {
  const count = await page.locator("button.attach > img.attach-img").count();
  for (let i = 0; i < count; i++) {
    const img = page.locator("button.attach > img.attach-img").nth(i);
    await img.scrollIntoViewIfNeeded();
    await page.waitForFunction((n) => { const x = document.querySelectorAll("button.attach > img.attach-img")[n]; return !x || (x.complete && x.naturalWidth > 0); }, i).catch(() => {});
  }
  return count;
}

export default async function attachCheck(browser) {
  const r = reporter("attach");
  const R = r.results;

  // ---- The API: references in the page JSON, bytes by reference, validation -------------------------------------------
  {
    const page = await served(browser, { extras: true, size: "desktop" });
    const api = await page.evaluate(async (sid) => {
      const text = await (await fetch("/api/tx?sid=" + sid)).text();
      const entries = JSON.parse(text).entries, images = entries.filter((e) => e.img).map((e) => ({ k: e.k, img: e.img }));
      const first = images[0]?.img[0];
      const head = async (url) => { const res = await fetch(url); return { status: res.status, type: res.headers.get("content-type"), nosniff: res.headers.get("x-content-type-options"), cache: res.headers.get("cache-control"), csp: res.headers.get("content-security-policy"), bytes: (await res.arrayBuffer()).byteLength }; };
      const url = (o, b, v, s = sid) => "/api/attachment?sid=" + encodeURIComponent(s) + "&o=" + o + "&b=" + b + "&v=" + v;
      const masked = images.flatMap((e) => e.img).find((i) => i.na);
      return {
        images, bytesInPage: /iVBORw0KGgo/.test(text), placeholders: /\[Image #\d+\]|\[image\]/.test(text),
        ok: first ? await head(url(first.o, first.b, first.v)) : null,
        textBlock: first ? await head(url(first.o, first.b + 1, first.v)) : null,
        otherSession: first ? await head(url(first.o, first.b, first.v, "harbor")) : null,
        otherVersion: first ? await head(url(first.o, first.b, "0123456789abcdef")) : null,
        masked: masked ? await head(url(masked.o, masked.b, "0123456789abcdef")) : null,
        size: first?.size,
      };
    }, SID);
    R.api = api;
    const refs = api.images.flatMap((e) => e.img);
    r.expect(api.images.length === 5 && refs.filter((i) => !i.na).length === 4 && refs.filter((i) => i.na).length === 1, "/api/tx names 4 servable images and 1 unavailable in 5 prompts: " + JSON.stringify(api.images));
    r.expect(refs.every((i) => i.na || (i.type === "image/png" && i.size > 0 && Number.isInteger(i.o) && Number.isInteger(i.b) && /^[0-9a-f]{16}$/.test(i.v) && [[180, 390], [320, 140]].some(([w, h]) => i.w === w && i.h === h))), "each image reference has its offset, block, type, size, version and the PNG's own width and height: " + JSON.stringify(refs));
    r.expect(!api.bytesInPage && !api.placeholders, "/api/tx holds no image bytes and no placeholder text");
    r.expect(api.ok?.status === 200 && api.ok.type === "image/png" && api.ok.nosniff === "nosniff" && /immutable/.test(api.ok.cache ?? "") && /private/.test(api.ok.cache ?? "") && /default-src 'none'/.test(api.ok.csp ?? "") && api.ok.bytes === api.size, "/api/attachment serves the PNG with its headers: " + JSON.stringify(api.ok));
    r.expect([api.textBlock, api.otherSession, api.otherVersion, api.masked].every((x) => x?.status === 404), "/api/attachment refuses a text block, another session, another version and the masked image: " + JSON.stringify([api.textBlock?.status, api.otherSession?.status, api.otherVersion?.status, api.masked?.status]));
    r.expect(page.errors.length === 0, "api: page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- The transcript and the sheet, at every size and scheme -----------------------------------------------------------
  for (const [size, dark] of [["phone", false], ["phone", true], ["desktop", false], ["desktop", true]]) {
    const tag = (size === "phone" ? "390" : "1280") + "-" + (dark ? "dark" : "light");
    const page = await served(browser, { extras: true, size, dark, path: "/s/claude/" + SID });
    // Nothing moves as the thumbnails load: with every image held back, each button's box is already the one it keeps.
    let release; const gate = new Promise((done) => { release = done; });
    await page.route(/\/api\/attachment\?/, async (route) => { await gate; await route.continue(); });
    await page.reload({ waitUntil: "domcontentloaded" }); await settled(page);
    await page.waitForFunction(() => document.querySelectorAll("button.attach").length >= 4);
    const boxes = () => page.evaluate(() => [...document.querySelectorAll("button.attach")].map((b) => { const box = b.getBoundingClientRect(); return [Math.round(box.width * 10) / 10, Math.round(box.height * 10) / 10]; }));
    const held = await page.evaluate(() => [...document.querySelectorAll("button.attach > img.attach-img")].filter((x) => !(x.complete && x.naturalWidth > 0)).length);
    const before = await boxes();
    release();
    const count = await loadAll(page);
    const after = await boxes();
    R[tag + "-boxes"] = { held, before, after };
    r.expect(held === 4, tag + ": the images were held back while the boxes were measured: " + held + " of 4 still loading");
    r.expect(JSON.stringify(before) === JSON.stringify(after), tag + ": a thumbnail's box changed when its image loaded: " + JSON.stringify({ before, after }));
    const c = await census(page);
    R[tag] = { count, thumbs: c.thumbs, chips: c.chips, bubbles: c.bubbles };
    r.expect(c.thumbs.length === 4 && c.thumbs.every((t) => t.inUser), tag + ": four thumbnails, each in a message bubble: " + JSON.stringify(c.thumbs.map((t) => t.inUser)));
    r.expect(c.thumbs.every((t) => t.loaded), tag + ": every thumbnail loaded: " + JSON.stringify(c.thumbs.map((t) => t.loaded)));
    r.expect(c.thumbs.every((t) => t.w <= 200.5 && t.h <= 160.5 && t.w > 20 && t.h > 20), tag + ": thumbnails are at most 200×160: " + JSON.stringify(c.thumbs.map((t) => [t.w, t.h])));
    r.expect(c.thumbs.every((t) => t.nh && Math.abs(t.w / t.h - t.nw / t.nh) < 0.03), tag + ": thumbnails keep their shape: " + JSON.stringify(c.thumbs.map((t) => [t.w, t.h, t.nw, t.nh])));
    r.expect(c.thumbs.every((t) => /^Attached image 1 \(PNG, \d+(\.\d)? (KB|MB|bytes)\)$/.test(t.alt)), tag + ": alt text names the image (the first of its message), its type and size: " + JSON.stringify(c.thumbs.map((t) => t.alt)));
    r.expect(c.thumbs.every((t) => /^\/api\/attachment\?sid=attach&o=\d+&b=\d+&v=[0-9a-f]{16}$/.test(t.src ?? "")), tag + ": each thumbnail's src is its /api/attachment URL: " + JSON.stringify(c.thumbs.map((t) => t.src)));
    r.expect(c.chips.length === 1 && c.chips[0].text === "Image not available" && c.chips[0].inUser, tag + ": the masked image is one quiet chip: " + JSON.stringify(c.chips));
    r.expect(!/\[image\]|\[Image #\d+\]/.test(c.text), tag + ": no \"[image]\" or \"[Image #N]\" text in the transcript");
    r.expect(c.text.includes("The top bar is bloated by the info on mobile") && c.text.includes("And on the wide screen too") && c.text.includes("A brief with a picture"), tag + ": the messages' text shows beside their images");
    const over = await overflow(page);
    r.expect(over === 0, tag + ": " + over + " elements past the screen's edge (1000+ means the page scrolls sideways)");
    await page.locator("button.attach").first().scrollIntoViewIfNeeded();
    await page.evaluate(() => window.scrollBy(0, -80));
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, "attach-" + tag + ".png") });
    // The image-only message and the masked image's chip, further down.
    await page.locator(".attach-na").first().scrollIntoViewIfNeeded();
    await page.waitForTimeout(150);
    await page.screenshot({ path: path.join(ENV.out, "attach-chip-" + tag + ".png") });

    // Open the first thumbnail, then close with Escape: focus returns to it.
    const first = page.locator("button.attach").first();
    await first.click();
    await page.waitForFunction(() => { const x = document.querySelector("dialog.image-viewer[open] img.attach-full"); return x && x.complete && x.naturalWidth > 0; });
    const open = await page.evaluate(() => {
      const d = document.querySelector("dialog.image-viewer[open]"), img = d.querySelector("img.attach-full"), box = img.getBoundingClientRect();
      return { label: d.getAttribute("aria-label"), alt: img.alt, inside: box.left >= -0.5 && box.top >= -0.5 && box.right <= innerWidth + 0.5 && box.bottom <= innerHeight + 0.5, w: box.width, h: box.height, focus: document.activeElement?.getAttribute("aria-label") };
    });
    R[tag].open = open;
    r.expect(open.inside && open.w > 40 && open.h > 40, tag + ": the sheet shows the image whole, inside the screen: " + JSON.stringify(open));
    r.expect(/^Attached image 1 \(PNG, /.test(open.label ?? "") && open.alt === open.label && open.focus === "Close", tag + ": the sheet is labelled with the image and focus is on Close: " + JSON.stringify(open));
    await page.screenshot({ path: path.join(ENV.out, "attach-open-" + tag + ".png") });
    await page.keyboard.press("Escape");
    await page.waitForFunction(() => !document.querySelector("dialog.image-viewer"));
    const back = await page.evaluate(() => document.activeElement?.matches("button.attach") && document.activeElement === document.querySelector("button.attach"));
    r.expect(back, tag + ": Escape closes the sheet and focus returns to the thumbnail");
    if (size === "phone") {
      await first.click();
      await page.waitForSelector("dialog.image-viewer[open]");
      await page.evaluate(() => history.back());
      await page.waitForFunction(() => !document.querySelector("dialog.image-viewer"));
      const stayed = await page.evaluate(() => location.pathname);
      r.expect(stayed === "/s/claude/" + SID, tag + ": the back gesture closes the sheet and stays on the session: " + stayed);
    }
    r.expect(page.errors.length === 0, tag + ": page errors " + page.errors.join(" | "));
    await page.context().close();
  }

  // ---- The embedding API fixture serves the image with the same headers ------------------------------------------------
  if (ENV.accountBase) {
    const page = await served(browser, { account: true, size: "desktop", path: "/s/claude/" + SID });
    const got = await page.evaluate(async (sid) => {
      const img = (await (await fetch("/api/tx?sid=" + sid)).json()).entries.find((e) => e.img)?.img[0];
      if (!img) return null;
      const res = await fetch("/api/attachment?sid=" + sid + "&o=" + img.o + "&b=" + img.b + "&v=" + img.v);
      return { status: res.status, type: res.headers.get("content-type"), nosniff: res.headers.get("x-content-type-options"), cache: res.headers.get("cache-control"), csp: res.headers.get("content-security-policy") };
    }, SID);
    R.embedding = got;
    r.expect(got?.status === 200 && got.type === "image/png" && got.nosniff === "nosniff" && /immutable/.test(got.cache ?? "") && /default-src 'none'/.test(got.csp ?? ""), "the embedding fixture serves the image with its own headers: " + JSON.stringify(got));
    await page.context().close();
  }
  return r.done();
}
