# Session discovery and inspection delivery

## Source and ownership

The baseline is `274f0b9bb43f65982863a22c177413c04d805740` on `main`.
The implementation is on `codex/session-inspection-design`. Both repositories
discover ui-design from dev-skills revision
`83ea88ffd8eb4d2abb47501d1458522fcc772e10`. Semon owns the shared Preact reader,
CSS roles, router and transcript controls. Hub owns machine enrollment and its
native domain pages. No shared skill change or managed projection edit was needed.

The bounded workflow is metadata discovery, selected native history, failure
inspection and return to the same list context. Catalog-v1 supports scoped
metadata search and ranged native records, not global conversation search or a
complete agent graph. The explicit compatibility reader retains its existing
complete-model capabilities.

## Design evidence

Review images now retain native source resolution: Stitch originals are 2560px
wide (downloaded with `=s0`), canvas exports use pen.dev `scale: 2`, and product
captures use Playwright `deviceScaleFactor: 2` at unchanged CSS viewports.
Desktop PNGs are 2560 × 1720 and phone PNGs are 780 × 1720; no low-resolution
bitmap was enlarged. Open the linked PNG at original size to inspect text.
[Filtered content detail](after/filtered-1280-detail.png),
[transcript detail](after/failure-1280-detail.png) and
[failed tool detail](after/tool-output-detail.png) omit unrelated chrome for reading.
The high-resolution baseline captures were reproduced with the baseline JS/CSS
against the same synthetic source. Original 1x captures remain in Git history.

The authoritative specification is [DESIGN.md](../../../.stitch/DESIGN.md).
Exact CSS values remain implementation authority; the specification defines
intent and reuse. The [existing workflow brief](../ai-design-workflow.md) links
these artifacts through the pinned ui-design skill.

Stitch project `4096612056644866749` generated two actual directions:

| Direction | Screen | Preview | Decision |
| --- | --- | --- | --- |
| A: search-first reader | `3e0ca02d8a604ba2932095cee5879661` | [Rendered A](../../../.stitch/designs/direction-a-0.png) | Selected: preserves reading width, existing host ownership and phone navigation |
| B: retained three-pane context | `ee33d0e2514041ef9eb45e91c917f401` | [Rendered B](../../../.stitch/designs/direction-b-0.png) | Deferred: useful simultaneous context, but narrower reading area and new pane/focus ownership |

The uploaded specification was used by Stitch's design-system workflow to create
`assets/4efd29b1bcee4cccbe2a54657836d9ce`. Returned design-system text is retained
as generation evidence in `.stitch/designs/stitch-system-output.md`. Its invented
Material colors and capability assumptions were rejected during reconciliation.
Metadata and returned HTML are in `.stitch/`; only synthetic records were uploaded.

[session-inspection.pen](session-inspection.pen) is a native schema 2.20 document,
edited and rendered with pen.dev CLI 0.3.10. It contains named frames, editable
text, reusable filters/session rows/tool panels and themed variables, with no
embedded screenshot stand-ins. Cloud used the supported headless CLI; no desktop
MCP synchronization is claimed.

| Frame | Rendered canvas |
| --- | --- |
| Desktop catalog, 1280 × 860 | [Nt3eb](canvas/Nt3eb.png) |
| Filtered phone, 390 × 860 | [gZYIY](canvas/gZYIY.png) |
| Compact reader, collapsed tool, 1280 × 860 | [vGVHj](canvas/vGVHj.png) |
| Expanded failure, 1280 × 860 | [nGo6u](canvas/nGo6u.png) |
| Loading, empty, error and focus | [Oo357](canvas/Oo357.png) |
| Dark catalog | [gbtK3](canvas/gbtK3.png) |

Post-Stitch changes restore exact shared roles, remove invented socket/connection
status and totals, separate list/reader states, keep contiguous rows, add native
Apply filters submission, and retain the single-line toolbar. Failure content was
reconstructed from the real sanitized harbor fixture. Existing transcript styling
and navigation remain shared components. Canvas navigation is schematic; it does
not authorize changes to shared chrome. The canvas is a reviewable intent model,
not a pixel-identical DOM export.

User review preferred the previous actual compact viewer over the mockup style.
The final UI preserves its original typography, type scale, search controls,
automatic form wrapping, page spacing, one toolbar and one reading column.
Inline labels remain visible. The result count, Clear filters and collapsed
Search scope share a metadata row, with active errors still visible.
The compact reader shows tools collapsed; the expanded failure frame is a separate
inspection state. Original Stitch A/B previews remain exploration evidence, not
the final layout. The final implementation uses the same compact geometry while
retaining inline labels, atomic submission and context restoration.
The canonical stylesheet before the catalog-specific extension is byte-for-byte
unchanged from the inspected baseline. [Style parity evidence](validation/preserved-style.json)
compares search controls against that baseline at 390/820/1280 in both themes;
family, size, line height, control height, padding, corners, gap and background
match. Geometry for the canvas was measured from the real rendered controls.
The [actual reader capture](after/reader-collapsed-1280.png) is the visual review
authority; the editable canvas describes its workflow and states.

## Result and screenshots

Search has persistent labels, reliable Enter/Apply submission and atomic filters.
The loaded count avoids claiming a global total. Scope explanation is a native
disclosure; active errors/loading remain visible. Clear filters resets once and
focuses search. Back restores source filters, row focus and list scroll.

Two defects found with actual records were fixed: retained metadata search now
reads current metadata before a history copy exists; missing original handoff
prompts and unprovenanced derived end markers no longer invalidate the transcript.
Missing content is explicitly unavailable, never fabricated or granted field-read
authority.

| Comparison | Before | After |
| --- | --- | --- |
| Desktop catalog | [Before](before/catalog-1280.png) | [After](after/catalog-1280.png) |
| Phone catalog | [Before](before/catalog-390.png) | [After](after/catalog-390.png) |
| Tablet catalog | [Before](before/catalog-820.png) | [After](after/catalog-820.png) |
| Harbor failure, desktop | [Before](before/failure-1280.png) | [After](after/failure-1280.png) |
| Harbor failure, phone | [Before](before/failure-390.png) | [After](after/failure-390.png) |

Failure baseline captures use the baseline's authored CSS/generated JS against
the same unchanged native response shape. This isolates the pre-existing parser
failure without rebuilding the backend. Other baseline captures came from the
original running UI. [Filtered phone](after/filtered-390.png) and
[filtered desktop](after/filtered-1280.png) show the selected canvas state.
The existing compatibility view was also exercised with nested activity and
an expanded real failure at both widths; [evidence](validation/compatibility.json)
and [desktop capture](after/compatibility-nested-1280.png) retain that separate
capability without claiming a complete graph in catalog-v1.

Visual review compared 390/1280 frames with actual screenshots: reading width,
16px phone gutters, persistent labels, stacked 44px phone controls, contiguous
rows and single-line Back match the design intent. The existing tool component
keeps its native sunken output and copy/full-output controls. Sidebar details and
natural message height are deliberately inherited. No user study or measured
usability gain is claimed.

## Reproducible preview

Use Node 22+ and Rust from the repository toolchain. From the repository root:

```sh
npm --prefix ui ci --no-audit --no-fund
npm --prefix ui run build
cargo build --locked -p semon-store --bin semon --features semon-sessions/test-clock
fixture_dir=$(mktemp -d)
fixture_now=$(node tests/ui/fixture.mjs "$fixture_dir" --extras)
SEMON_TEST_NOW="$fixture_now" target/debug/semon sessions --serve \
  --listen 127.0.0.1:8081 --claude-home "$fixture_dir/claude" \
  --claude-json "$fixture_dir/.claude.json" --codex-home "$fixture_dir/codex" \
  --copilot-home "$fixture_dir/copilot" --proc-root "$fixture_dir/proc" \
  --cache "$fixture_dir/index.json"
```

Open the local token URL printed by your server; do not publish it. Search harbor,
open harbor, expand the failed Bash call, inspect complete output, then use Back.
Use `compat=1` for the existing complete relationship view. The delivered workspace
preview runs on port 8082; authentication tokens are intentionally excluded from
the artifacts. The fixture uses synthetic session messages, paths and identities.

## Validation

Executed successfully on the implementation tree:

- `cargo fmt --all -- --check`, locked all-target clippy and `cargo test --locked`
  (including 529 session-crate tests; existing ignored cases remain ignored).
- UI formatting, typecheck, architecture, semantic design policy (0 violations),
  generated bundle freshness and 89 unit tests.
- 12 catalog browser tests, including atomic search, empty/retry recovery,
  filter/scroll/focus restoration and phone/desktop overflow checks.
- Real bounded producer test: a 110,029-byte scalar read over two text pages,
  390/1280 light/dark text audits, keyboard paging, retained history after source
  loss, and readable harbor/q-codex/h-failed records. No global model restoration.
  [Raw served evidence](validation/served/evidence.json) contains observations,
  not a comparative performance benchmark.
- Existing gallery (30 cases) and compact gallery (6 cases) passed with unchanged
  thresholds and representative keyboard/touch/reduced-motion checks.
- Agent synchronization passed against the existing pinned architecture.

The changed browser cases are in `ui/tests/catalog-viewer-browser.mjs` and
`catalog-served.mjs`; run the same commands used by `.github/workflows/ui.yml`.
Hub consumer validation and its reviewed-pin adoption boundary are recorded in
the companion Hub delivery. CI remains the final exact-head merge gate.

## Continuing alignment

Future Codex UI work reads DESIGN.md and existing ownership contracts first,
reuses semantic CSS roles/components, updates the native canvas when intent
changes, validates representative states at matching viewports and records
intentional deviations in the PR. Review pixel diffs before updating references.
These are additions to the existing repository brief and pinned skill discovery,
not a second instruction system or a new dependency package.
