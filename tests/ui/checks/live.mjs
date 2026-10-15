// Real native source publication, visibility and selected request ownership.
import { pathToFileURL } from 'node:url';
import { launch } from '../lib.mjs';
import liveCatalog from './live-catalog.mjs';

export default liveCatalog;

// CI invokes this file directly. Re-exporting alone never executes its oracle.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const browser = await launch();
  try {
    if (!(await liveCatalog(browser))) process.exitCode = 1;
  } finally {
    await browser.close();
  }
}
