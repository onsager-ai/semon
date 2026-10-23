# Sessions as the default application entry

The October 19, 2026 product decision removes Home and opens the existing Sessions
page by default. It explicitly rejects moving Needs you, Working or inbox sections
into Sessions. The shared sidebar now has Sessions, Analytics and Machines.

The implementation preserves query parameters and fragments when normalizing `/`
to `/sessions`, including during history navigation. Shared workspace metadata
continues to populate Recent; the Home metadata API is not retired. Hub owns the
matching authenticated HTTP redirect and native/no-JavaScript page removal.

## Matched captures from the original implementation

These pairs were captured for the October 19 candidate
`1c2cfedbda7edae39d8231aad2dc14238ecf8f35`. The reference bundle was built from
`0d6bae2d4588993735cb879c871f666260b0c9d4`
using the same synthetic Catalog fixture, stylesheet, viewport and theme as the
candidate. These are real browser renders, not image-generation or Site previews.
Both captures use the existing Sessions route to isolate the navigation change;
the browser tests separately verify entering through `/` and returning through it.

| State | Reference | Candidate | Disposition |
| --- | --- | --- | --- |
| 1280px dark Sessions | [Before](before-1280-dark.png) | [After](after-1280-dark.png) | Intentional removal of Home; Sessions becomes the first sidebar item. |
| 390px light Sessions | [Before](before-390-light.png) | [After](after-390-light.png) | Preserve the existing compact Sessions content and closed drawer. |

The committed pairs cover 1280px dark and 390px light. Desktop navigation is
intentionally shorter; the mobile Sessions controls and rows retain their
presentation. These historical pairs illustrate the original change. Acceptance
of the refreshed branch uses its own CI captures at 390/820/1280px in both themes,
including the required interaction states. Complete-application visual
differences follow the existing comparator and exact-artifact review procedure.

## Qualification of the refreshed branch

The October 23 refresh incorporates current main and migrates obsolete Home
assertions onto retained Sessions, Analytics, machine and transcript surfaces.
Its required checks and their exact-head results are tracked on
[Semon #448](https://github.com/onsager-ai/semon/pull/448/checks).

- Rust, agent configuration and native-usage checks retain their owning CI gates.
- UI foundation checks cover formatting, architecture, design, types, unit tests
  and generated-bundle freshness. CI rebuilds the Viewer from authored sources.
- The full Viewer browser aggregate retains ordering, navigation, lifecycle,
  accessibility, live updates, error recovery, transport, served-producer and
  visual qualification. Root entry covers query/fragment retention and history;
  native forms, workspace Recent and retired-response isolation remain covered.
- [Hub #279](https://github.com/onsager-ai/semon-hub/pull/279) owns the paired
  authenticated redirect, PostgreSQL-backed journeys and native/no-JavaScript
  qualification after adopting the merged Source revision.

Historical focused-test counts and the two matched pairs do not certify a new
head. Final acceptance binds green CI and reviewed visual artifacts to the
delivered revision. No baseline thresholds or paused capabilities change.
