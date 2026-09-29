// Solid's JSX compiles to DOM templates only through its Babel preset (babel-preset-solid), which neither esbuild nor Bun
// can run natively. This bundles the Solid sample with esbuild's JS API under Node, or with Bun.build under Bun, each with
// a plugin that runs Babel on every .tsx file. Usage: <node|bun> solid.mjs <esbuild|bun> <min 1|0> <out, from the bench root>
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { transformAsync } from "@babel/core";
import solid from "babel-preset-solid";
import typescript from "@babel/preset-typescript";

const [bundler, min, out] = process.argv.slice(2);
const root = fileURLToPath(new URL("../..", import.meta.url));
const entry = path.join(root, "src/solid/main.tsx"), outfile = path.resolve(root, out), minify = min === "1";

async function compile(file) {
  const source = await readFile(file, "utf8");
  const result = await transformAsync(source, { filename: file, babelrc: false, configFile: false, presets: [[solid, { generate: "dom" }], [typescript, { onlyRemoveTypeImports: true }]] });
  return { contents: result.code, loader: "js" };
}
const plugin = { name: "solid-babel", setup: (build) => build.onLoad({ filter: /\.tsx$/ }, (args) => compile(args.path)) };

if (bundler === "esbuild") {
  const esbuild = await import("esbuild");
  await esbuild.build({ entryPoints: [entry], bundle: true, format: "iife", platform: "browser", target: "es2022", minify, outfile, plugins: [plugin], logLevel: "warning" });
} else if (bundler === "bun") {
  const extra = JSON.parse(process.env.BUN_EXTRA ?? "{}");
  const result = await Bun.build({ entrypoints: [entry], target: "browser", format: "iife", minify, outdir: path.dirname(outfile), naming: path.basename(outfile), plugins: [plugin], ...extra });
  if (!result.success) {
    for (const log of result.logs) console.error(log);
    process.exit(1);
  }
} else {
  console.error("usage: solid.mjs <esbuild|bun> <1|0> <out>");
  process.exit(2);
}
