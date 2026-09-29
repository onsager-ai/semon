#!/usr/bin/env bash
# The TSX bench for docs/design/tsx.md, run by .github/workflows/tsx-bench.yml and nowhere else: it downloads bundlers and
# bundles with them, which is CI-only work. For each bundler (esbuild, Bun, Vite) and runtime (h, Preact, Solid, lit-html)
# it bundles the same sample (src/) as one IIFE and records the install footprint, cold and warm bundle times, output
# sizes, the viewer's banned-string test run against each bundle, a single-file transform (no bundling) and tsc --noEmit.
# Writes res/results.md. A failed build or type check is recorded and the rest still run; the script then exits 1.
set -uo pipefail
cd "$(dirname "$0")"

ESBUILD_VERSION=0.28.2
BUN_VERSION=1.4.2
RUNTIMES=(h preact solid lit)
# legacy: today's served script (tooltip.js, select.js and viewer.js, joined as shell::VIEWER_JS joins them), bundled as it is.
TARGETS=(legacy "${RUNTIMES[@]}")
BUNDLERS=(esbuild bun vite)
RUNS=${RUNS:-10}
FAILED=()
rm -rf bin out res && mkdir -p bin out res

now_ns() { date +%s%N; }
since_ms() { echo $((($(now_ns) - $1) / 1000000)); }
bytes() { du -sb "$@" | awk '{ s += $1 } END { print s }'; }
mb() { awk -v b="$1" 'BEGIN { printf "%.1f MB", b / 1048576 }'; }
kb() { awk -v b="$1" 'BEGIN { printf "%.1f KB", b / 1024 }'; }
noshell() { [[ $1 =~ ^[A-Z_]+= ]] || echo -N; } # hyperfine runs a command with no env prefix without a shell
group() { echo "::group::$*"; }
endgroup() { echo "::endgroup::"; }

# ---- Installs, each timed and in its own directory, so each footprint is its own ---------------------------------------
group "install"
t=$(now_ns)
curl -fsSL "https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-$ESBUILD_VERSION.tgz" | tar -xz -C bin --strip-components=2 package/bin/esbuild || exit 1
esbuild_ms=$(since_ms "$t") esbuild_bytes=$(bytes bin/esbuild)
t=$(now_ns)
curl -fsSL -o bin/bun.zip "https://github.com/oven-sh/bun/releases/download/bun-v$BUN_VERSION/bun-linux-x64.zip" && unzip -q -j bin/bun.zip bun-linux-x64/bun -d bin && rm bin/bun.zip || exit 1
bun_ms=$(since_ms "$t") bun_bytes=$(bytes bin/bun)
npm_install() { # dir -> "ms bytes"
  local t; t=$(now_ns)
  npm install --prefix "$1" --no-audit --no-fund --no-package-lock --loglevel=error >&2 || return 1
  echo "$(since_ms "$t") $(bytes "$1/node_modules")"
}
got=$(npm_install .) || exit 1; read -r runtimes_ms runtimes_bytes <<<"$got"
got=$(npm_install pkg/vite) || exit 1; read -r vite_ms vite_bytes <<<"$got"
got=$(npm_install pkg/vite-solid) || exit 1; read -r vitesolid_ms vitesolid_bytes <<<"$got"
got=$(npm_install pkg/babel) || exit 1; read -r babel_ms babel_bytes <<<"$got"
got=$(npm_install pkg/ts) || exit 1; read -r ts_ms ts_bytes <<<"$got"
TSC=pkg/ts/node_modules/.bin/tsc
pkgver() { node -p "require('./$1/package.json').version"; }
V_ESBUILD=$(bin/esbuild --version) V_BUN=$(bin/bun --version) V_VITE=$(pkgver pkg/vite/node_modules/vite) V_ROLLDOWN=$(pkgver pkg/vite/node_modules/rolldown)
V_TS=$($TSC --version | sed 's/^Version //') V_NODE=$(node --version) V_PREACT=$(pkgver node_modules/preact) V_SOLID=$(pkgver node_modules/solid-js) V_LIT=$(pkgver node_modules/lit-html)
V_BABEL=$(pkgver pkg/babel/node_modules/@babel/core) V_BPS=$(pkgver pkg/babel/node_modules/babel-preset-solid) V_VPS=$(pkgver pkg/vite-solid/node_modules/vite-plugin-solid)
endgroup
group "probe bun"; bash probe-bun.sh; endgroup

# ---- Type checks: tsc --noEmit per runtime (none of the bundlers type-check) -------------------------------------------
declare -A TSC_MS
for rt in "${RUNTIMES[@]}"; do
  group "tsc $rt"
  if $TSC -p "src/$rt/tsconfig.json"; then
    hyperfine --style=none -N --warmup 1 --runs 3 --export-json "res/tsc-$rt.json" "$TSC -p src/$rt/tsconfig.json" >/dev/null
    TSC_MS[$rt]=$(jq -r '.results[0].mean * 1000 | round' "res/tsc-$rt.json")
  else
    TSC_MS[$rt]="failed"; FAILED+=("tsc-$rt")
  fi
  endgroup
done

# ---- Bundles ------------------------------------------------------------------------------------------------------------
SRC=../../../crates/semon-sessions/src
{ cat "$SRC/tooltip.js"; echo; cat "$SRC/select.js"; echo; cat "$SRC/viewer.js"; } > out/legacy-entry.js
entry() { case "$1" in legacy) echo out/legacy-entry.js ;; lit) echo src/lit/main.ts ;; *) echo "src/$1/main.tsx" ;; esac; }
build_cmd() { # bundler runtime min(1|0) out
  local b=$1 rt=$2 min=$3 out=$4 flag="" dir=vite
  [ "$min" = 1 ] && flag="--minify"
  [ "$rt" = solid ] && dir=vite-solid
  case "$b" in
    esbuild) if [ "$rt" = solid ]; then echo "node pkg/babel/solid.mjs esbuild $min $out"; else echo "bin/esbuild $(entry "$rt") --bundle --format=iife --platform=browser --target=es2022 --log-level=warning $flag --outfile=$out"; fi ;;
    bun) if [ "$rt" = solid ]; then echo "bin/bun pkg/babel/solid.mjs bun $min $out"; else echo "bin/bun build $(entry "$rt") --target=browser --format=iife $flag --outfile=$out"; fi ;;
    vite) echo "RT=$rt ENTRY=$(entry "$rt") MIN=$min OUT=$out node pkg/$dir/node_modules/vite/bin/vite.js build --config pkg/$dir/vite.config.mjs --logLevel warn" ;;
  esac
}
# The needles crates/semon-sessions/src/viewer.rs bans from VIEWER_JS (the_page_has_no_inline_script_style_or_html_injection).
NEEDLES=("innerHTML" "outerHTML" "insertAdjacentHTML" "document.write" "eval(" "new Function" 'setAttribute("style"' "cssText" ".title =" ".title=" 'setAttribute("title"')
scan() { # file -> "needle×n, ..." or "none"
  local f=$1 hits=() n
  for needle in "${NEEDLES[@]}"; do
    n=$({ grep -o -F -- "$needle" "$f" || true; } | wc -l)
    [ "$n" -gt 0 ] && hits+=("\`$needle\`×$n")
  done
  n=$({ grep -o -E '(^|[^A-Za-z0-9_$])title:' "$f" || true; } | wc -l)
  [ "$n" -gt 0 ] && hits+=("\`title:\`×$n")
  if [ ${#hits[@]} -eq 0 ]; then echo "none"; else local IFS=","; echo "${hits[*]}" | sed 's/,/, /g'; fi
}
declare -A COLD WARM MIN GZ DEV HITS OPEN
for rt in "${TARGETS[@]}"; do
  for b in "${BUNDLERS[@]}"; do
    id="$b-$rt" min="out/$b-$rt.min.js" dev="out/$b-$rt.js"
    cmd=$(build_cmd "$b" "$rt" 1 "$min")
    group "$id: $cmd"
    t=$(now_ns)
    if bash -c "$cmd" && [ -s "$min" ]; then
      COLD[$id]=$(since_ms "$t")
      hyperfine --style=none $(noshell "$cmd") --warmup 2 --runs "$RUNS" --export-json "res/$id.json" "$cmd" >/dev/null
      WARM[$id]=$(jq -r '.results[0] | "\(.mean * 1000 | round) ± \(.stddev * 1000 | round)"' "res/$id.json")
      bash -c "$(build_cmd "$b" "$rt" 0 "$dev")" || FAILED+=("$id-dev")
      MIN[$id]=$(stat -c %s "$min") GZ[$id]=$(gzip -9c "$min" | wc -c) DEV[$id]=$(stat -c %s "$dev" 2>/dev/null || echo 0)
      HITS[$id]=$(scan "$dev")
      OPEN[$id]="$(head -c 1 "$dev")$(head -c 1 "$min")"
      echo "cold ${COLD[$id]} ms, warm ${WARM[$id]} ms, ${MIN[$id]} B min, ${GZ[$id]} B gzip, ${DEV[$id]} B unminified; banned: ${HITS[$id]}"
    else
      COLD[$id]="failed" WARM[$id]="failed" MIN[$id]=0 GZ[$id]=0 DEV[$id]=0 HITS[$id]="-" OPEN[$id]="-"
      FAILED+=("$id")
    fi
    endgroup
  done
done

# ---- A single-file transform, no bundling (the local check R-20260929-16 allows) ---------------------------------------
declare -A ONE
for f in src/preact/SessionTree.tsx src/solid/SessionTree.tsx src/shared/tooltip.ts; do
  for b in esbuild bun; do
    if [ "$b" = esbuild ]; then cmd="bin/esbuild $f --jsx=preserve --log-level=warning"; else cmd="bin/bun build --no-bundle $f"; fi
    key="$b $f"
    if bash -c "$cmd" >/dev/null; then
      hyperfine --style=none -N --warmup 2 --runs "$RUNS" --export-json res/one.json "$cmd" >/dev/null
      ONE[$key]=$(jq -r '.results[0] | "\(.mean * 1000 | round) ± \(.stddev * 1000 | round)"' res/one.json)
    else
      ONE[$key]="failed"; FAILED+=("one-$b-$f")
    fi
  done
done

# ---- Results ------------------------------------------------------------------------------------------------------------
{
  echo "## TSX bench"
  echo
  echo "Runner: $(nproc) vCPU, $(lscpu | sed -n 's/^Model name: *//p' | head -1). Node $V_NODE. esbuild $V_ESBUILD, Bun $V_BUN, Vite $V_VITE (Rolldown $V_ROLLDOWN), vite-plugin-solid $V_VPS, Babel $V_BABEL with babel-preset-solid $V_BPS, TypeScript $V_TS. Preact $V_PREACT, Solid $V_SOLID, lit-html $V_LIT. Warm times: mean ± σ of $RUNS runs after 2 warm-ups (hyperfine). Cold: the first run after install."
  echo
  echo "### Install footprint"
  echo
  echo "| What | Needs Node | On disk | Install time |"
  echo "|---|---|---|---|"
  echo "| esbuild $V_ESBUILD, the standalone binary from the npm registry | no | $(mb "$esbuild_bytes") | ${esbuild_ms} ms |"
  echo "| Bun $V_BUN, the standalone binary from its release | no | $(mb "$bun_bytes") | ${bun_ms} ms |"
  echo "| Vite $V_VITE (npm) | yes | $(mb "$vite_bytes") | ${vite_ms} ms |"
  echo "| Vite + vite-plugin-solid (npm) | yes | $(mb "$vitesolid_bytes") | ${vitesolid_ms} ms |"
  echo "| Babel + babel-preset-solid + esbuild's npm package (Solid under esbuild or Bun) | yes, or Bun | $(mb "$babel_bytes") | ${babel_ms} ms |"
  echo "| TypeScript $V_TS (npm) | yes | $(mb "$ts_bytes") | ${ts_ms} ms |"
  echo "| The runtimes: preact, solid-js, lit-html (npm) | - | $(mb "$runtimes_bytes") | ${runtimes_ms} ms |"
  echo
  echo "Runtime packages on disk: preact $(mb "$(bytes node_modules/preact)"), solid-js $(mb "$(bytes node_modules/solid-js)"), lit-html $(mb "$(bytes node_modules/lit-html)")."
  echo
  echo "### Bundle time (ms, minified IIFE)"
  echo
  echo "| Runtime | esbuild cold | esbuild warm | Bun cold | Bun warm | Vite cold | Vite warm |"
  echo "|---|---|---|---|---|---|---|"
  for rt in "${TARGETS[@]}"; do
    echo "| $rt | ${COLD[esbuild-$rt]} | ${WARM[esbuild-$rt]} | ${COLD[bun-$rt]} | ${WARM[bun-$rt]} | ${COLD[vite-$rt]} | ${WARM[vite-$rt]} |"
  done
  echo
  echo "### Output size (minified / gzip -9 of minified / unminified)"
  echo
  echo "legacy is today's served script, shell::VIEWER_JS (tooltip.js, select.js and viewer.js): $(kb "$(stat -c %s out/legacy-entry.js)") as served, $(kb "$(gzip -9c out/legacy-entry.js | wc -c)") gzipped. The other rows are the sample (the tooltip, the session tree, a clamped brief) for each runtime; h has no runtime, so each other row's excess over h is its runtime."
  echo
  echo "| Runtime | esbuild | Bun | Vite |"
  echo "|---|---|---|---|"
  for rt in "${TARGETS[@]}"; do
    row="| $rt"
    for b in "${BUNDLERS[@]}"; do row+=" | $(kb "${MIN[$b-$rt]}") / $(kb "${GZ[$b-$rt]}") / $(kb "${DEV[$b-$rt]}")"; done
    echo "$row |"
  done
  echo
  echo "### viewer.rs's banned strings in each unminified bundle, and its first character (unminified, minified)"
  echo
  echo "| Runtime | esbuild | Bun | Vite |"
  echo "|---|---|---|---|"
  for rt in "${TARGETS[@]}"; do
    row="| $rt"
    for b in "${BUNDLERS[@]}"; do row+=" | ${HITS[$b-$rt]} (\`${OPEN[$b-$rt]}\`)"; done
    echo "$row |"
  done
  echo
  echo "### One file, transformed without bundling (ms)"
  echo
  echo "| File | esbuild | Bun | Vite |"
  echo "|---|---|---|---|"
  for f in src/preact/SessionTree.tsx src/solid/SessionTree.tsx src/shared/tooltip.ts; do
    echo "| \`$f\` | ${ONE[esbuild $f]} | ${ONE[bun $f]} | no standalone transform |"
  done
  echo
  echo "### tsc --noEmit per runtime (ms, mean of 3)"
  echo
  echo "| h | preact | solid | lit |"
  echo "|---|---|---|---|"
  echo "| ${TSC_MS[h]} | ${TSC_MS[preact]} | ${TSC_MS[solid]} | ${TSC_MS[lit]} |"
  if [ ${#FAILED[@]} -gt 0 ]; then echo; echo "Failed: ${FAILED[*]}"; fi
} > res/results.md
cat res/results.md
[ ${#FAILED[@]} -eq 0 ]
