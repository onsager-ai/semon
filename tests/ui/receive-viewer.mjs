// The transport fixture uses the receive listener for push and browser reads.
import assert from 'node:assert/strict';
import { ENV, launch, served } from './lib.mjs';

const browser = await launch();
try {
  const page = await served(browser, { size: 'desktop' });
  assert.equal(new URL(page.url()).searchParams.has('t'), false);
  const cookies = await page.context().cookies();
  const session = cookies.find((cookie) => cookie.name === 'semon_session');
  assert.ok(session?.httpOnly);
  assert.equal(session.sameSite, 'Strict');
  const response = await page.request.get(ENV.base + '/api/session-sources');
  assert.equal(response.status(), 200);
  await page.screenshot({ path: ENV.out + '/receive-viewer.png', fullPage: true });
  assert.deepEqual(page.errors, []);
  await page.context().close();
} finally {
  await browser.close();
}
console.log('receive Viewer: read-token login, clean URL, cookie and received sources passed');
