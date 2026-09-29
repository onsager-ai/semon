// The Vite build of one runtime's sample, as a single IIFE. bench.sh sets RT (h, preact, solid or lit), MIN (1 to minify)
// and OUT (the bundle's path, relative to the bench root). vite.config.mjs here and in ../vite-solid use it.
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));
const JSX = { h: { runtime: "classic", pragma: "h" }, preact: { runtime: "automatic", importSource: "preact" } };

export function config(plugins = []) {
  const rt = process.env.RT, out = path.resolve(root, process.env.OUT);
  return {
    root,
    logLevel: "warn",
    publicDir: false,
    plugins,
    ...(JSX[rt] ? { oxc: { jsx: JSX[rt] } } : {}),
    build: {
      outDir: path.dirname(out),
      emptyOutDir: false,
      minify: process.env.MIN === "1",
      target: "es2022",
      reportCompressedSize: false,
      copyPublicDir: false,
      lib: { entry: path.join(root, "src", rt, rt === "lit" ? "main.ts" : "main.tsx"), formats: ["iife"], name: "SemonBench", fileName: () => path.basename(out) },
    },
  };
}
