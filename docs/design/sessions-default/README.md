# Sessions as the default application entry

The October 19, 2026 product decision removes Home and opens the existing Sessions
page by default. It explicitly rejects moving Needs you, Working or inbox sections
into Sessions. The shared sidebar now has Sessions, Analytics and Machines.

The implementation preserves query parameters and fragments when normalizing `/`
to `/sessions`, including during history navigation. Shared workspace metadata
continues to populate Recent; the Home metadata API is not retired. Hub owns the
matching authenticated HTTP redirect and native/no-JavaScript page removal.

## Actual implementation captures

The reference bundle was built from `0d6bae2d4588993735cb879c871f666260b0c9d4`
using the same synthetic Catalog fixture, stylesheet, viewport and theme as the
candidate. These are real browser renders, not image-generation or Site previews.
Both captures use the existing Sessions route to isolate the navigation change;
the browser tests separately verify entering through `/` and returning through it.

| State | Reference | Candidate | Disposition |
| --- | --- | --- | --- |
| 1280px dark Sessions | [Before](before-1280-dark.png) | [After](after-1280-dark.png) | Intentional removal of Home; Sessions becomes the first sidebar item. |
| 390px light Sessions | [Before](before-390-light.png) | [After](after-390-light.png) | Preserve the existing compact Sessions content and closed drawer. |

Reviewed both image families. Desktop navigation is intentionally shorter; the
mobile Sessions controls and rows retain their presentation. Six current captures
cover 390/820/1280px in light/dark. Existing complete-application pixel references
have not been overwritten or approved by these captures; their Home navigation
expectations require review under the current product decision.

## Validation

- `npm --prefix ui test`: 154 tests passed.
- Focused `catalog-viewer-browser.mjs` checks: 18 tests passed, covering root entry,
  query/fragment retention, history navigation, native forms, workspace Recent and
  retired-response isolation. All three viewport sizes and both themes are used
  for root entry.
- `cargo test --locked -p semon-sessions shell::tests`: 9 tests passed.
- Type checking, formatting, design policy, architecture and generated bundle
  freshness passed. The generated Viewer bundle is rebuilt from authored sources.

Full served-producer, PostgreSQL and pixel-acceptance qualification are not claimed
by this focused validation. No baseline thresholds or paused capabilities change.
