// Launches one browser, runs every ported check in order, prints each report, and exits 1 if any failed.
//
//   SEMON_BASE / SEMON_TOKEN / SEMON_NOW   the primary fixture's served viewer (required)
//   SEMON_EXTRA_BASE / SEMON_EXTRA_TOKEN   the --extras fixture's served viewer (required: md's synthetic pass, bar's
//                                          table check and extras.mjs run on it)
//   SEMON_UI_OUT                          where reports and screenshots go (default: ./out)
import { launch } from "../lib.mjs";
import full from "./full.mjs";
import viewerCheck from "./viewer.mjs";
import turnsCheck from "./turns.mjs";
import barCheck from "./bar.mjs";
import mdCheck from "./md.mjs";
import homeCheck from "./home.mjs";
import timelineCheck from "./timeline.mjs";
import checkReal from "./check-real.mjs";
import extras from "./extras.mjs";
import shellCheck from "./shell.mjs";

const checks = [
  ["home", homeCheck],
  ["bar", barCheck],
  ["turns", turnsCheck],
  ["viewer", viewerCheck],
  ["timeline", timelineCheck],
  ["md", mdCheck],
  ["full", full],
  ["check-real", checkReal],
  ["extras", extras],
  ["shell", shellCheck],
];

const browser = await launch();
let failed = false;
for (const [name, fn] of checks) {
  console.log("---- " + name + " ----");
  try {
    const ok = await fn(browser);
    if (!ok) failed = true;
  } catch (e) {
    console.error("== " + name + ": threw " + (e?.stack ?? e));
    failed = true;
  }
}
await browser.close();
if (failed) process.exit(1);
