// Launches one browser, runs every ported check in order, prints each report, and exits 1 if any failed.
//
//   SEMON_BASE / SEMON_TOKEN / SEMON_NOW   the primary fixture's served viewer (required)
//   SEMON_EXTRA_BASE / SEMON_EXTRA_TOKEN   the --extras fixture's served viewer (required: md's synthetic pass, bar's
//                                          table check, extras.mjs, attach.mjs and sidebar.mjs run on it)
//   SEMON_ACCOUNT_BASE / SEMON_ACCOUNT_TOKEN   the embedding API fixture used by extras.mjs
//   SEMON_UI_OUT                          where reports and screenshots go (default: ./out)
import { launch } from "../lib.mjs";
import full from "./full.mjs";
import viewerCheck from "./viewer.mjs";
import turnsCheck from "./turns.mjs";
import barCheck from "./bar.mjs";
import mdCheck from "./md.mjs";
import homeCheck from "./home.mjs";
import analyticsCheck from "./analytics.mjs";
import checkReal from "./check-real.mjs";
import extras from "./extras.mjs";
import attachCheck from "./attach.mjs";
import shellCheck from "./shell.mjs";
import namesCheck from "./names.mjs";
import sidebarCheck from "./sidebar.mjs";
import tokensCheck from "./tokens.mjs";
import scrollbarsCheck from "./scrollbars.mjs";
import switchCheck from "./switch.mjs";
import embedCheck from "./embed.mjs";
import tooltipCheck from "./tooltip.mjs";
import detailsCheck from "./details.mjs";
import selectCheck from "./select.mjs";
import filtersCheck from "./filters.mjs";

const checks = [
  ["home", homeCheck],
  ["bar", barCheck],
  ["turns", turnsCheck],
  ["viewer", viewerCheck],
  ["analytics", analyticsCheck],
  ["md", mdCheck],
  ["full", full],
  ["check-real", checkReal],
  ["extras", extras],
  ["attach", attachCheck],
  ["shell", shellCheck],
  ["names", namesCheck],
  ["sidebar", sidebarCheck],
  ["tokens", tokensCheck],
  ["scrollbars", scrollbarsCheck],
  ["switch", switchCheck],
  ["embed", embedCheck],
  ["tooltip", tooltipCheck],
  ["details", detailsCheck],
  ["select", selectCheck],
  ["filters", filtersCheck],
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
