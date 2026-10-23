# PR474 integration resolution

Main integrated: 01c8e42c0e3a6a271014190a089c9ce8aa8fd762.
PR parent: 262f5b0f221a90dc2a913df21f9d63e571d7f92f.

Conflicts:
- `.github/workflows/rust.yml`: keep PR's failure-specific formatting artifact and main's upload compression benchmark. No gate removed or relaxed.
- `crates/semon-sessions/src/viewer.generated.js`: retain the PR bundle provisionally; consume the exact CI-generated review patch before completion. Authored `catalogViewer.tsx` combines main's bounded recovery with PR's lifecycle tooltip.
- `tests/ui/visual-acceptance.json`: preserve all 114 PR bindings, including three reviewed Last-turn regions; all 111 shared records differ from main only in fingerprint/rationale. Append main's provenance to those shared rationales, preserving each original review/evidence field. Fresh CI fingerprints require exact-artifact qualification; old acceptance is not integrated-head acceptance.

Version reconciliation: common ancestor e0925cd2 and main both use parser cache28 and slot recipe6; only the PR introduces cache29. Retain cache29 and recipe6; no competing version bump exists. Main's storage schema31 and all proof/publication/archive/selected-recovery changes remain. Migration uses main's proof Snapshot and PR's source-owned lifecycle fold.

Validation is CI-only. No local cargo build/check/test/clippy/fmt, Node tests, typecheck or bundle generation. Thermal log absent; direct live k10temp readings before/after merge: 31.375C/31.375C, later inspection32.25C. Stop CPU work at85C; no CPU work without live telemetry. Independent acceptance and Manager review remain required; Worker never merges. Keep PR Draft and #430/#426 open.

First integrated run: Viewer foundation39933503091 correctly rejected the provisional stale bundle. Applied its exact `ui-generated-review` artifact12491424897 without running a local generator; patch changes only viewer.generated.js. Restored the prior CI-only advisory binding audit for the now114 existing records (684 actual artifact digests), with no registry writes or raw-gate changes. It will distinguish unchanged bindings from genuine new captures before any fingerprint update.

Integrated visual proof: run39933749307 artifact12492823634 (`viewer-visual-binding-audit`) passed all114 identities and684 actual image/evidence digests, zero unregistered states/errors. Tested merge aad343270b57d4a3bef78e4f87e35013d9080afe and delivered1b3acbaa97a0d7c64bcab88f5330564d2ad013e9 share tree13b844511ea291aea0f6c96018b93aa3b4688e0e. Exact current fingerprint f56e1d0679e135754f7535c39a1b4967482b6e60e2ccce6cc85c9547577f7839. Update only candidateFingerprint for114 existing entries; original review fields, references, artifact digests, ratios and thresholds stay unchanged. Remove completed one-off advisory audit; raw comparator remains required.

Worker actual-image inspection (not independent acceptance): visual artifact12492818658 from that same run; inspected all9 reference/candidate/diff images for phone-light, phone-dark and desktop-light deps menu, plus all6 preservation images for390-light Harbor and1280-dark Atlas. Menu differences show only the previously reviewed Last-turn row and downstream shift; Harbor/Atlas show the previously approved passive-control notice removal and resulting reading-dock visibility. CI proves all captures byte-identical to their existing reviewed bindings. No new appearance approval is asserted. Images downloaded/extracted locally with live sensor32.125C before/after, no browser or local build/test used.

## Second integration: PR481 main movement (2026-10-23)

Starting PR head af4a675c56914e521b7a862afd0e9870427cfd36; actual fetched main
cf1ec9c4cdf8ac102d8952cd0f75b6fbee8a395e. Merge, no rebase/force. Prior CI and
image acceptance above are historical and cannot qualify this revision.

Conflict decisions:
- events.rs: preserve main's pub(crate) CACHE_VERSION visibility and PR's29.
  Ancestor01c8e42c/maincf1ec9c4 both28; PRaf4a675c29. Slot recipe6 on all three.
  Schema33 from481 remains; no competing parser or slot recipe bump.
- native_usage/migration.rs -> native_usage/repair.rs: preserve main's complete
  removal of legacy usage conversion, model rederivation and migration storage.
  Observe source-qualified lifecycle in current receipt-bound tree repair instead,
  using main's boolean copied-usage identity check. Existing tree labels checkpoint
  the reducer. New regression crosses512-record repair slices and checks no retired
  migration table appears. Native Codex legacy carriers stay supported inputs.
- reprice.rs: main's parser_version helper plus parser29 assertion.
- model/tests.rs: retain main's deletion of obsolete direct-formatter cache-upgrade
  compatibility test. Current rebuild and source replacement lifecycle tests remain;
  conformance workflow correctly uses main's current-storage refusal entrypoint.
- viewer.generated.js: PR bundle retained only provisionally; CI foundation must
  regenerate from merged canonical ui/src (including main's overviewPages change).
- visual-acceptance.json:111 main bindings and114 PR bindings have identical
  shared evidence/review fields; differences only fingerprints/rationale. Preserve
  all114 PR bindings, including3 Last-turn regions. No approval/threshold/reference
  change; fresh artifacts must qualify every fingerprint change. No advisory
  audit workflow/script reintroduced.

Docs reconcile schema33 whole-index invalidation/parser29 and current source repair,
without restoring removed Semon compatibility. No CPU-heavy local checks, builds,
formatting, tests or generation. Live k10temp31C before merge and30.875C after bounded
resolution; temps.log absent. Stop CPU work>=85C/no telemetry. Every local read is
bounded to one file under10MB. CI and fresh independent acceptance remain required;
Worker does not merge. #430/#426 remain open.

First second-integration CI: merge24727306, Viewer39939272365 artifact12494699399
provided the exact generated bundle patch (only viewer.generated.js). Foundation
passed format/architecture/design/types and163/164 tests; sole failure was stale
bundle. Applied artifact, no local generation. Rust39939272515 artifact12495556351
provided formatting of the added regression only. Applied exact patch. Exact-head
conformance passed78 parser tests and9 existing lifecycle tests; the added repair
fixture incorrectly stayed under the1MiB synchronous repair ceiling, so its
expectation of deferred repair failed. Increased sanitized filler above1MiB;
kept all checkpoint/phase/no-migration assertions. Upstream pinned reducer passed.
No weakening of product or raw gates. New CI required after this correction.

Follow-up0acad350 CI: Viewer39939638981 foundation passed (including types,
all164 UI tests, bundle freshness and infrastructure checks). Rust39939639163
both exact-head Semon and upstream conformance jobs passed: corrected current-source
repair regression and all remaining lifecycle/native/selected-output contracts pass.
Only fmt requested a wrap of the enlarged filler macro; applied exact artifact
12495252871. No local formatter/test/generator used. Final new runtime still needs
actual visual artifact qualification; retained fingerprints are explicitly stale
until that evidence exists, never an inferred pass.
