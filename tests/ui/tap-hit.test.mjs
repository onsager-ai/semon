import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { chromium } from "playwright";
import { measure } from "./checks/taps.mjs";

let browser;
before(async () => { browser = await chromium.launch({ args: ["--disable-gpu"] }); });
after(async () => { await browser?.close(); });

async function inspect(body) {
  const page = await browser.newPage({ viewport: { width: 300, height: 300 } });
  try {
    await page.setContent('<style>body{margin:0}button{width:44px;height:44px;padding:0;border:0}</style>' + body);
    return await page.evaluate(measure, { selector: "button", min: 44 });
  } finally { await page.close(); }
}

test("a fixed layer cannot hide a control in another fixed layer", async () => {
  const result = await inspect('<div style="position:fixed;inset:0"><button style="position:absolute;left:20px;top:20px">Tap</button></div><div style="position:fixed;left:20px;top:20px;width:44px;height:44px;z-index:2">Cover</div>');
  assert.ok(result.misses.length > 0);
});

test("a clipping ancestor with no scroll range cannot hide a target's lost edge", async () => {
  const result = await inspect('<div style="position:absolute;left:50px;top:20px;width:44px;height:44px;overflow:hidden"><button style="position:absolute;left:-30px;top:0">Tap</button></div>');
  assert.ok(result.misses.some((miss) => miss.side === "left"));
});

test("a corner overlap is caught even when all four edge midpoints are clear", async () => {
  const result = await inspect('<button style="position:absolute;left:20px;top:20px">Tap</button><div style="position:absolute;left:57px;top:57px;width:8px;height:8px">Cover</div>');
  assert.ok(result.misses.some((miss) => miss.side === "bottom-right"));
  assert.equal(result.misses.filter((miss) => ["top", "bottom", "left", "right"].includes(miss.side)).length, 0);
});

test("closed native details expose only their direct summary controls", async () => {
  const result = await inspect('<details><summary><button>Summary</button></summary><button>Closed</button><details open><summary>Nested</summary><button>Nested closed parent</button></details></details><details open><summary>Open</summary><button>Visible</button></details>');
  assert.deepEqual(result.items.map((item) => item.text), ["Summary", "Visible"]);
});

test("an open drawer scopes controls while a native modal takes precedence", async () => {
  const page = await browser.newPage({ viewport: { width: 300, height: 300 } });
  try {
    await page.setContent('<style>button{width:44px;height:44px}</style><body class="drawer-open"><aside class="sidebar"><button>Drawer</button></aside><button>Behind drawer</button><dialog><button>Modal</button></dialog>');
    const args = { selector: "button", min: 44 };
    const drawer = await page.evaluate(measure, args);
    assert.deepEqual(drawer.items.map((item) => item.text), ["Drawer"]);
    await page.locator("dialog").evaluate((dialog) => dialog.showModal());
    const modal = await page.evaluate(measure, args);
    assert.deepEqual(modal.items.map((item) => item.text), ["Modal"]);
  } finally { await page.close(); }
});
