/**
 * Render the mockup screens named in shots.json at phone and desktop sizes,
 * in light and dark mode, and write screenshots plus a diagnostics report.
 * This shooter is for CI only; do not run it on a local machine.
 *
 * Each manifest `eval` runs in the page after load. The sample keeps `go()`
 * private in its script closure, so its evals click stable navigation controls
 * (which call the mockup's own router). If a mockup exposes its router, an eval
 * can call that function directly. Keep evals short so the manifest is easy to
 * extend when the design branch updates the mockup.
 */

import { readFile, mkdir, writeFile, appendFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { chromium } from "playwright";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const MANIFEST = path.join(ROOT, "tests/ui/shots.json");
const OUT = path.join(ROOT, "tests/ui/out/shots");
const FONT_WAIT_MS = 10_000;
const SIZES = [
  { name: "390", viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true, deviceScaleFactor: 2 },
  { name: "1280", viewport: { width: 1280, height: 860 }, isMobile: false, hasTouch: false, deviceScaleFactor: 1 },
];
const SCHEMES = ["light", "dark"];

function manifestError(message) {
  throw new Error(`Invalid screenshot manifest: ${message}`);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

async function validateManifest(manifest) {
  if (!Array.isArray(manifest) || manifest.length === 0) manifestError("expected a non-empty JSON list");
  const files = new Set();

  for (const [fileIndex, entry] of manifest.entries()) {
    if (!isObject(entry)) manifestError(`entry ${fileIndex + 1} must be an object`);
    if (Object.keys(entry).some((key) => !["file", "shots"].includes(key))) {
      manifestError(`entry ${fileIndex + 1} has an unknown property`);
    }
    if (typeof entry.file !== "string" || !entry.file || path.isAbsolute(entry.file) || entry.file.split(/[\\/]/).includes("..")) {
      manifestError(`entry ${fileIndex + 1} needs a repo-relative file path without '..'`);
    }
    if (files.has(entry.file)) manifestError(`duplicate file entry: ${entry.file}`);
    files.add(entry.file);
    if (!Array.isArray(entry.shots) || entry.shots.length === 0) manifestError(`${entry.file}: shots must be a non-empty list`);

    const names = new Set();
    for (const [shotIndex, shot] of entry.shots.entries()) {
      const label = `${entry.file}, shot ${shotIndex + 1}`;
      if (!isObject(shot)) manifestError(`${label} must be an object`);
      if (Object.keys(shot).some((key) => !["name", "hash", "query", "eval", "wait"].includes(key))) {
        manifestError(`${label} has an unknown property`);
      }
      if (typeof shot.name !== "string" || !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(shot.name)) {
        manifestError(`${label} needs a lowercase slug name`);
      }
      if (names.has(shot.name)) manifestError(`${entry.file}: duplicate shot name ${shot.name}`);
      names.add(shot.name);
      if (shot.hash !== undefined && (typeof shot.hash !== "string" || !shot.hash.startsWith("#"))) {
        manifestError(`${label}: hash must be a string beginning with '#'`);
      }
      if (shot.query !== undefined && (typeof shot.query !== "string" || !shot.query.startsWith("?"))) {
        manifestError(`${label}: query must be a string beginning with '?'`);
      }
      if (shot.eval !== undefined) {
        if (typeof shot.eval !== "string") manifestError(`${label}: eval must be a JavaScript string`);
        try {
          // Parse the page expression here so malformed manifest code is caught before rendering.
          new Function(shot.eval);
        } catch (error) {
          manifestError(`${label}: invalid eval JavaScript (${error.message})`);
        }
      }
      if (shot.wait !== undefined && (!Number.isInteger(shot.wait) || shot.wait < 0)) {
        manifestError(`${label}: wait must be a non-negative integer`);
      }
    }

    const filePath = path.resolve(ROOT, entry.file);
    if (!filePath.startsWith(ROOT + path.sep)) manifestError(`${entry.file}: path must stay inside the repository`);
    try {
      await readFile(filePath);
    } catch (error) {
      if (error.code === "ENOENT" || error.code === "EISDIR") {
        throw new Error(`Screenshot manifest file is missing: ${entry.file}`);
      }
      throw error;
    }
  }
}

function slugForFile(file) {
  return file.replace(/\\/g, "/").replace(/\.[^/.]+$/, "").replace(/[^a-zA-Z0-9]+/g, "-").replace(/^-|-$/g, "").toLowerCase();
}

function pageUrl(filePath, shot) {
  const url = pathToFileURL(filePath);
  if (shot.query) url.search = shot.query.slice(1);
  if (shot.hash) url.hash = shot.hash.slice(1);
  return url.href;
}

function display(value) {
  return String(value ?? "").replaceAll("|", "\\|").replace(/[\r\n]+/g, " ");
}

async function waitForFonts(page) {
  let timer;
  try {
    await Promise.race([
      page.evaluate(() => document.fonts?.ready?.then(() => true) ?? Promise.resolve(true)),
      new Promise((resolve) => { timer = setTimeout(() => resolve(false), FONT_WAIT_MS); }),
    ]);
  } catch {
    // A page that fails to evaluate still gets reported and photographed if possible.
  } finally {
    clearTimeout(timer);
  }
}

function addPageErrors(page, record) {
  page.on("pageerror", (error) => record.pageErrors.push(error.message));
  page.on("console", (message) => {
    if (message.type() === "error") record.consoleErrors.push(message.text());
  });
}

async function renderCase(browser, entry, shot, size, scheme, fileSlug) {
  const record = {
    file: entry.file,
    shot: shot.name,
    viewport: size.name,
    colorScheme: scheme,
    horizontalOverflow: null,
    pageErrors: [],
    consoleErrors: [],
    renderErrors: [],
    screenshots: [],
  };
  const contextOptions = {
    ...size,
    colorScheme: scheme,
    reducedMotion: "reduce",
  };
  delete contextOptions.name;

  let context;
  try {
    context = await browser.newContext(contextOptions);
    const page = await context.newPage();
    addPageErrors(page, record);
    const filePath = path.resolve(ROOT, entry.file);
    try {
      await page.goto(pageUrl(filePath, shot), { waitUntil: "domcontentloaded", timeout: FONT_WAIT_MS });
    } catch (error) {
      record.renderErrors.push(`navigation: ${error.message}`);
    }
    await waitForFonts(page);
    if (shot.eval) {
      try {
        await page.evaluate((source) => eval(source), shot.eval);
      } catch (error) {
        record.renderErrors.push(`eval: ${error.message}`);
      }
    }
    await page.waitForTimeout(shot.wait ?? 300).catch((error) => record.renderErrors.push(`wait: ${error.message}`));
    try {
      record.horizontalOverflow = await page.evaluate(() => document.documentElement.scrollWidth > window.innerWidth);
    } catch (error) {
      record.renderErrors.push(`overflow check: ${error.message}`);
    }

    const directory = path.join(OUT, fileSlug);
    await mkdir(directory, { recursive: true });
    const stem = `${shot.name}-${size.name}-${scheme}`;
    for (const [suffix, fullPage] of [["", false], ["-full", true]]) {
      const absolutePath = path.join(directory, `${stem}${suffix}.png`);
      try {
        await page.screenshot({ path: absolutePath, fullPage, animations: "disabled" });
        record.screenshots.push(path.relative(ROOT, absolutePath).replace(/\\/g, "/"));
      } catch (error) {
        record.renderErrors.push(`screenshot ${path.basename(absolutePath)}: ${error.message}`);
      }
    }
  } catch (error) {
    record.renderErrors.push(`browser setup: ${error.message}`);
  } finally {
    await context?.close().catch((error) => record.renderErrors.push(`context close: ${error.message}`));
  }
  return record;
}

function makeSummary(report) {
  const lines = [
    "## Mockup screenshots",
    "",
    "Screenshots and the full diagnostics report are in the `mockup-shots` artifact.",
    "",
    "| File | Shot | Viewport | Scheme | Sideways scroll | Page errors | Console errors | PNGs |",
    "| --- | --- | ---: | --- | --- | ---: | ---: | ---: |",
  ];
  for (const record of report.cases) {
    lines.push(`| ${display(record.file)} | ${display(record.shot)} | ${record.viewport} | ${record.colorScheme} | ${record.horizontalOverflow === null ? "unknown" : record.horizontalOverflow ? "yes" : "no"} | ${record.pageErrors.length} | ${record.consoleErrors.length} | ${record.screenshots.length} |`);
  }
  lines.push("", `PNG files written: ${report.cases.reduce((sum, record) => sum + record.screenshots.length, 0)}.`);
  if (report.environmentError) lines.push("", `Browser setup error: ${display(report.environmentError)}`);
  return lines.join("\n") + "\n";
}

async function main() {
  let manifest;
  try {
    manifest = JSON.parse(await readFile(MANIFEST, "utf8"));
    await validateManifest(manifest);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
    return;
  }

  await mkdir(OUT, { recursive: true });
  const report = { generatedAt: new Date().toISOString(), cases: [] };
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
  } catch (error) {
    report.environmentError = error.message;
  }

  if (browser) {
    try {
      for (const entry of manifest) {
        const fileSlug = slugForFile(entry.file);
        for (const shot of entry.shots) {
          for (const size of SIZES) {
            for (const scheme of SCHEMES) {
              report.cases.push(await renderCase(browser, entry, shot, size, scheme, fileSlug));
            }
          }
        }
      }
    } catch (error) {
      // Rendering problems are diagnostics, not visual gates.
      report.environmentError = error.message;
    } finally {
      await browser.close().catch((error) => { report.environmentError ??= error.message; });
    }
  }

  await writeFile(path.join(OUT, "report.json"), JSON.stringify(report, null, 2) + "\n");
  const summary = makeSummary(report);
  if (process.env.GITHUB_STEP_SUMMARY) await appendFile(process.env.GITHUB_STEP_SUMMARY, summary);
  else process.stdout.write(summary);
}

await main();
