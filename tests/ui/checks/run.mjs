// Runs a validated CI group (or all checks locally), records durations, and fails if any assertion fails.
//
//   SEMON_BASE / SEMON_TOKEN / SEMON_NOW   the primary fixture's served viewer (required)
//   SEMON_EXTRA_BASE / SEMON_EXTRA_TOKEN   the --extras fixture's served viewer (required: md's synthetic pass, bar's
//                                          table check, extras.mjs, attach.mjs and sidebar.mjs run on it)
//   SEMON_ACCOUNT_BASE / SEMON_ACCOUNT_TOKEN   the embedding API fixture used by extras.mjs
//   SEMON_UI_OUT                          where reports and screenshots go (default: ./out)
import fs from "node:fs";
import path from "node:path";
import { performance } from "node:perf_hooks";
import { selectChecks } from "../suite-plan.mjs";
import { ENV, launch } from "../lib.mjs";
import full from "./full.mjs";
import viewerCheck from "./viewer.mjs";
import turnsCheck from "./turns.mjs";
import barCheck from "./bar.mjs";
import mdCheck from "./md.mjs";
import agentFontCheck from "./agentfont.mjs";
import homeCheck from "./home.mjs";
import analyticsCheck from "./analytics.mjs";
import checkReal from "./check-real.mjs";
import extras from "./extras.mjs";
import attachCheck from "./attach.mjs";
import shellCheck from "./shell.mjs";
import embedSidebarCheck from "./embedsidebar.mjs";
import namesCheck from "./names.mjs";
import hiconsCheck from "./hicons.mjs";
import sidebarCheck from "./sidebar.mjs";
import tokensCheck from "./tokens.mjs";
import scrollbarsCheck from "./scrollbars.mjs";
import switchCheck from "./switch.mjs";
import embedCheck from "./embed.mjs";
import tooltipCheck from "./tooltip.mjs";
import detailsCheck from "./details.mjs";
import selectCheck from "./select.mjs";
import filtersCheck from "./filters.mjs";
import breakdownCheck from "./breakdown.mjs";
import tracebriefCheck from "./tracebrief.mjs";
import runView from "./runview.mjs";
import deepLinkLive from "./deeplink-live.mjs";
import codexThinkingCheck from "./codex-thinking.mjs";
import tapsCheck from "./taps.mjs";

import backlogCheck from "./viewer-backlog.mjs";
import modelsCheck from "./models.mjs";
import stepNamesCheck from "./stepnames.mjs";
import sidebarFocusCheck from "./sidebarfocus.mjs";

import signalsCheck from "./signals.mjs";

const checks = [
  ["runview", runView],
  ["signals", signalsCheck],
  ["viewer-backlog", backlogCheck],
  ["models", modelsCheck],
  ["stepnames", stepNamesCheck],
  ["sidebarfocus", sidebarFocusCheck],
  ["deeplink-live", deepLinkLive],
  ["codex-thinking", codexThinkingCheck],
  ["home", homeCheck],
  ["bar", barCheck],
  ["turns", turnsCheck],
  ["viewer", viewerCheck],
  ["analytics", analyticsCheck],
  ["md", mdCheck],
  ["agentfont", agentFontCheck],
  ["full", full],
  ["check-real", checkReal],
  ["extras", extras],
  ["attach", attachCheck],
  ["shell", shellCheck],
  ["embedsidebar", embedSidebarCheck],
  ["names", namesCheck],
  ["hicons", hiconsCheck],
  ["sidebar", sidebarCheck],
  ["tokens", tokensCheck],
  ["scrollbars", scrollbarsCheck],
  ["switch", switchCheck],
  ["embed", embedCheck],
  ["tooltip", tooltipCheck],
  ["details", detailsCheck],
  ["select", selectCheck],
  ["filters", filtersCheck],
  ["breakdown", breakdownCheck],
  ["tracebrief", tracebriefCheck],
  ["taps", tapsCheck],
];

const selected = selectChecks(checks, process.argv.slice(2));
const browser = await launch();
const timings = {};
let failed = false;
for (const [name, fn] of selected) {
  console.log("---- " + name + " ----");
  const started = performance.now();
  try {
    const ok = await fn(browser);
    if (!ok) failed = true;
  } catch (e) {
    console.error("== " + name + ": threw " + (e?.stack ?? e));
    failed = true;
  } finally {
    timings[name] = Math.round(performance.now() - started);
    console.log(`TIME ${name}: ${timings[name]} ms`);
  }
}
await browser.close();
fs.writeFileSync(path.join(ENV.out, "check-durations.json"), JSON.stringify(timings, null, 2) + "\n");
console.log("Check durations (ms): " + JSON.stringify(timings));
if (failed) process.exit(1);
