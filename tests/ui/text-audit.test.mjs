import { test } from 'node:test';
import assert from 'node:assert/strict';
import { launch } from './lib.mjs';
import { auditText } from './text-audit.mjs';

test('rendered text audit catches size, contrast and different copies; ignores hidden/disabled text', async () => {
  const browser = await launch();
  try {
    const page = await browser.newPage();
    await page.setContent('<style>body{background:#fff;color:#191d1b;font:14px sans-serif}.tiny{font-size:11px}.low{color:#8a918d}.hidden{display:none}.same{color:#666}.dark{background:#666}</style><p>Readable</p><p class="tiny">Too small</p><p class="low">Low contrast</p><p class="same">Repeated</p><div class="dark"><p class="same">Repeated</p></div><p class="tiny hidden">Hidden</p><button class="tiny low" disabled>Disabled</button>');
    const result = await auditText(page);
    assert.equal(result.smallCount, 1); assert.equal(result.lowCount, 2);
    assert.ok(result.low.some(row => row.text === 'Repeated'));
    assert.ok(!result.small.some(row => row.text === 'Hidden' || row.text === 'Disabled'));
    await page.setContent('<style>body{background:#fff;color:#000}.large{color:#777;font-size:24px}.bold{color:#777;font-size:19px;font-weight:700}</style><p class="large">Large text</p><p class="bold">Bold text</p>');
    assert.equal((await auditText(page)).lowCount, 0, 'AA large text threshold is 3:1');
  } finally { await browser.close(); }
});
