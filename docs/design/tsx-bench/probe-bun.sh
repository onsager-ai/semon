#!/usr/bin/env bash
# Temporary: which Bun flags make it resolve the production builds of lit-html and solid-js.
set -uo pipefail
cd "$(dirname "$0")"
mkdir -p out/probe
lit() { bin/bun build src/lit/main.ts --target=browser --format=iife "$@" >/dev/null 2>&1; }
count() { { grep -o -F -- "$1" "$2" || true; } | wc -l; }
lit --outfile=out/probe/default.js && echo "cli default: DEV_MODE $(count DEV_MODE out/probe/default.js)"
lit --production --outfile=out/probe/production.js && echo "cli --production: DEV_MODE $(count DEV_MODE out/probe/production.js)"
NODE_ENV=production lit --outfile=out/probe/nodeenv.js && echo "cli NODE_ENV=production: DEV_MODE $(count DEV_MODE out/probe/nodeenv.js)"
lit --conditions=production --outfile=out/probe/cond.js && echo "cli --conditions=production: DEV_MODE $(count DEV_MODE out/probe/cond.js)"
for extra in '{}' '{"conditions":["production"]}' '{"define":{"process.env.NODE_ENV":"\"production\""}}' '{"env":"disable"}'; do
  if BUN_EXTRA="$extra" bin/bun pkg/babel/solid.mjs bun 0 out/probe/solid.js >/dev/null 2>&1; then echo "api $extra: registerGraph $(count registerGraph out/probe/solid.js)"; else echo "api $extra: failed"; fi
done
if NODE_ENV=production bin/bun pkg/babel/solid.mjs bun 0 out/probe/solid.js >/dev/null 2>&1; then echo "api NODE_ENV=production: registerGraph $(count registerGraph out/probe/solid.js)"; fi
