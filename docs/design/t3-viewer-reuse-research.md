# Reusing T3 Code for session viewing

Research, 2026-11-09. The owner decision below amended the Viewer scope in the [ASI direction](asi-direction.md), which is authoritative. This note records the evidence behind that decision. It authorizes no implementation. No product code was changed, nothing was deployed or imported, and no live sessions were inspected.

## Current guidance

### Owner decision (2026-11-09)

The owner likes T3's UI/UX but does not want Semon to depend on T3. They chose standalone, read-only browsing of Semon-stored history over an analytics-plus-evidence-only Viewer. The [ASI direction](asi-direction.md) Viewer decision records the accepted scope; this section summarizes it.

- **Independence.** Semon works fully with no T3 installed, running or reachable. An "Open in T3" link is optional and appears only for a verified mapping.
- **Sidebar.** Semon keeps its existing sidebar/navigation. No T3 sidebar redesign is planned.
- **Transcript.** Semon keeps its read-only transcript over its own durable history. The existing catalog/transcript code is the starting point, not removal scope. T3's session-log (transcript) presentation is the reference, and borrowed patterns are reimplemented in Semon's Preact UI under the [ownership contract](viewer-ownership.md). No React/Effect port and no T3 state or server.
- **Licensing.** Copied code, styles or assets carry T3's MIT notice plus any per-asset licenses (see [License and assets](#license-and-assets)). No T3 branding.
- **Custody.** Bodies stay on the machine or the operator's receiver. Hub stays analytics-only, with no transcript browser.
- **Out of scope.** No composer, control, runtime or SSH.

### Transcript reuse finding

T3's timeline logic is not framework-free. At the pinned revision:

- [`MessagesTimeline.logic.ts`][timeline-logic] imports `@t3tools/client-runtime` (work-log presentation, user-message and subagent state), T3 contracts, Effect and zustand.
- [`TimelineWorkEntryRow.logic.ts`][work-entry-logic] imports `@t3tools/client-runtime` work-log presentation, T3 shared tool-activity helpers and mdast/unified types.

Copying either would pull in T3's normalization layer. Reimplement the presentation in Semon over its own records instead, using T3 as the visual and interaction reference.

**Candidate presentation targets.** These come from upstream file names and user docs only (for example `TimelineWorkGroupRows`, `TimelineToolActivityIcon`, `TimelineMinimap`, `timelineScrollAnchoring` and [thread-sidebar.md][sidebar-doc]). They are **not runtime-verified**: nobody has seen them running for this research. Confirm each one against a running T3 or its rendered output before treating it as a target.

- Consecutive tool calls collapsed into a work group, with icons and a duration.
- Per-tool labels and icons, with the full command and output on expansion.
- Collapsed thinking previews.
- Distinct user and assistant rows.
- A minimap of prompts.
- Find with match navigation.
- Scroll anchoring and position restore.
- Subagent rows that open the child.

Semon's renderer already has keyed turns, field expansion, selected-text navigation, reading anchors and child runs ([ui/README.md](../../ui/README.md)). The gap is mostly presentation.

### Proposed next experiment (not authorized)

**Proposed, not authorized; implementation needs separate approval.** Build a Storybook comparison of Semon's current transcript beside a T3-style presentation, on synthetic Claude Code and Codex fixtures, following [the AI design workflow](ai-design-workflow.md).

Acceptance criteria:

- The owner agrees the visual and interaction targets first, after the candidate targets above are confirmed against T3's actual behavior.
- It stays within Semon's Preact, HTML-sink and ownership rules and adds no React or T3 dependency.
- Unavailable or truncated fields are shown explicitly, and only synthetic data is used.
- Validation runs in CI. No local cargo/node builds, tests or project-wide typechecks. Check current thermal readings before and after any allowed CPU work, stop at 85 °C or higher, and do not start without readings.

### What stays Semon-owned

- Durable raw/index/analytics and labels, plus ingestion, redaction and custody.
- Native identities and recorded lineage.
- Pricing, StepFacts and shared/exclusive accounting.
- Query, SQL and FTS.
- Authentication and namespace authorization.
- Analytics charts and cohorts.
- Bounded evidence reads, provenance, generation checks and explicit partial/unavailable states.
- The existing sidebar/navigation and the read-only catalog/transcript.

The approved query routes are still specification or implementation-lane work; this note does not claim they have shipped.

Semon does not build a composer, control, runtime delivery or SSH. Removing code requires checking its consumers and passing the applicable gates in a separate authorized change. It never authorizes deleting logs, credentials, history or services.

## Evidence

### Revision and evidence standard

- **Canonical upstream:** [pingdotgg/t3code](https://github.com/pingdotgg/t3code), linked by the official [t3.codes](https://t3.codes/) site. GitHub reports it is not a fork.
- **Inspected upstream `main`:** [`c6ca6056309904f89b90b6055d797973be0a0939`](https://github.com/pingdotgg/t3code/commit/c6ca6056309904f89b90b6055d797973be0a0939), committed 2026-11-09 07:29:24 UTC.
- **Semon at initial inspection:** checkout and remote main at [`6e8a6b6007f8cfb1c0185f911d0b1cc9dd725e5c`](https://github.com/onsager-ai/semon/commit/6e8a6b6007f8cfb1c0185f911d0b1cc9dd725e5c).
- **"Verified"** below means inspected source, schemas or documentation at that revision, not a runtime test. No builds, tests, browser sessions or dependency installation ran. Local temperatures were checked, and source reads and searches were bounded per file. No private native logs, databases or credentials were read.
- **Local tools are not evidence.** The local T3 tools and this installation's behavior say nothing about the upstream implementation; the claims cite public upstream files. Upstream contains multi-environment navigation, peer links and MCP, but that does not show this installed build has the same revision, settings or grants. Nothing private or locally modified is proposed for copying.

### Feature and ownership comparison

| Need | Verified upstream T3 behavior | Semon position |
| --- | --- | --- |
| Session sidebar/navigation | Project/environment grouping; pinned, active and settled threads; search and message-match navigation; archived-thread surfaces. [Sidebar docs][sidebar-doc], [implementation][sidebar]. | Semon keeps its existing sidebar/navigation over its own multi-machine history. T3's thread catalog covers only T3-held threads and is not adopted. |
| Transcript rendering | Virtualized message timeline, tool activity, attachments, find navigation, citation selection and history paging for T3-owned records. [Timeline][timeline], [contracts][contracts]. | Semon keeps its read-only transcript over stored history, including sources absent from T3. T3 is the presentation reference ([current guidance](#transcript-reuse-finding)). |
| Evidence anchors | Environment/thread links. Assistant citations use T3 message IDs, UTF-16 offsets, quote text, prefix and suffix. [Links][links], [citation schema][citation], [navigation][citation-nav]. | No inspected contract maps an ASI step plus source/generation to an exact T3 item. Imported ordinal IDs and text matching do not establish one. Semon's [activity anchors](session-activity-analytics.md) validate native identity and generations, and may explicitly refuse navigation. |
| Identity and lineage | Environment-scoped app threads, provider instances and native thread references, parent/root threads, fork/subagent relations. [Contracts][contracts], [remote architecture][remote]. | Semon preserves namespace/harness/native-session identity, provenance, recorded lineage and exclusive/shared usage ownership. T3 app-thread IDs are different identities. Native import creates a root thread with a null parent and does not reconstruct source lineage. |
| Local historical sessions | Native discovery and import for Claude Code, Codex and Pi. Discovery uses a 30-day file-mtime window and keeps at most 200 conversation messages (the first user message plus the most recent history). [Scanner][scanner], [import schema][import-schema]. | Bounded convenience import, not durable full-fidelity history. Only text blocks are taken. Imported messages have empty attachments and null native item, provider-turn and parent-item references. Tool evidence and original message identity are not preserved. [Importer][importer]. |
| Synchronized / old history | Remote clients connect to the environment that holds its T3 state. Selected T3 thread histories page from persisted projections. [Architecture][architecture], [remote][remote]. | The inspected import API has no Semon receiver or archive input. It takes a T3 project ID, scans configured provider homes and writes T3 state. Codex discovery visits `sessions/YYYY/MM/DD`, not Semon archives. An offline origin's Semon-retained copy is not viewable in T3. |

These limits apply to native-session import, not to every T3-created thread; T3's own orchestration keeps richer tool and lineage records. Its V1 migration is a separate mechanism with documented omissions, not a generic Semon history importer.

### Framework fit and upstream tracking

The inspected [web package][web-package] is a private application package with no exported viewer-library API. It uses React, TanStack Router, Effect/client-runtime state, Tailwind, Base UI and Legend List. Semon uses Preact/TSX and deterministic esbuild assets embedded by Rust, with strict HTML-sink, inline-style and lifecycle ownership contracts ([ui/README.md](../../ui/README.md)).

- The application [sidebar][sidebar] imports Effect atoms, client-runtime stores, environment state, TanStack routing, DnD and thread commands.
- The [timeline][timeline] imports thread state, approvals, diff/canvas contexts, citations and virtual-list behavior.
- Even the lower-level [UI sidebar][sidebar-primitive] imports application UI helpers and uses inline styles.

Preact compatibility is untested and would not remove these ownership differences. File size alone is not a cost estimate; what matters is what each component imports and mutates.

Upstream's [README][readme] says large feature contributions are not currently accepted, so recheck that policy before proposing any upstream change. Pin any copied file to a revision and track its dependency closure, local patches and security fixes.

## License and assets

The pinned [LICENSE][license] is MIT, copyright **2026 T3 Tools Inc.** It permits reuse, including modification and distribution, provided the copyright and permission notice accompany copies or substantial portions. That clears T3's own source code. It does not clear the whole application or every asset under MIT.

Upstream generates separate third-party notices from its production/optional dependency closures and bundled module IDs. Its [license documentation][license-doc] and [exceptions/asset inventory][license-config] list, among others:

- adapted MIT Office viewer code;
- CC0 sounds;
- Apache-2.0 components;
- OFL-1.1 KaTeX fonts with reserved font names.

Anything copied must carry the upstream MIT notice plus the license, NOTICE, attribution and font conditions for what it actually redistributes. Keep provenance for adaptations. Do not copy unrelated sounds, fonts, logos or provider marks, and do not imply T3 endorsement. The root MIT license grants no trademark or hosted-service rights.

No complete transitive package audit was done, because nothing is proposed for distribution. Any future copying needs that audit for the exact files, including their own license headers and upstream patches. Recreating a design pattern without copying its code or assets redistributes nothing.

## Authorization and custody

These apply to the optional "Open in T3" link.

- **Semon checks stay authoritative.** Semon authorization, namespace binding, Origin checks and source-generation validation govern evidence. A historical record or a T3 link grants no runtime authority.
- **T3 has its own grants.** T3 has independent environment authentication and per-RPC permissions. Its documentation says `orchestration:operate` can start full-access agents, while MCP read-only access grants only orchestration reads. [Auth][auth].
- **A link is never an action.** Do not exchange Semon credentials for T3 grants. A link must not trigger import, resume, pairing, control or synchronization.
- **Prefer a direct target.** Use an explicitly configured direct or local T3 target. A hosted T3 chooser is optional, not the default, because even identifiers can reveal information outside the operator's boundary.
- **A link carries identity only.** It contains opaque identity, never credentials, raw paths, snippets or commands. T3's assistant-citation format includes quote text and surrounding context; base64 in a URL fragment does not make that body-free. Do not generate such links from Hub or use them to carry ASI evidence.

Semon's accepted [store](asi-store.md) and [query](asi-query.md) specifications keep raw/index bodies on the machine or the operator's receiver. Hub receives only body-free analytics and categorical labels, and `search`/`view` refuse there. A Hub analytics row can say evidence is unavailable or name an explicitly configured operator destination. It must not fetch or store bodies, or silently launch an origin agent to recover them.

## Historical: superseded pre-decision recommendation

> **Superseded 2026-11-09 by the owner decision above.** Kept as a record of the analysis. It is not current guidance: Semon now keeps standalone read-only browsing, its sidebar/navigation and its transcript, and does not rely on T3 for browsing. The evidence sections above remain current.

Before the decision, the research recommended using T3 itself for general browsing, including its sidebar, while Semon kept only its analytics shell and a minimal evidence view, plus an optional verified "Open in T3" link. It also recommended against copying T3's sidebar or transcript stack. That last point still holds for code, per the reuse finding above.

| Option | Assessment at the time |
| --- | --- |
| 1. Use/link into T3 | Lowest UI maintenance: T3 handles its own sidebar/rendering upgrades. Was preferred for sessions already in T3. |
| 2. Copy selected components behind an adapter | Not a narrow adapter, given the coupling above. Was deferred. |
| 3. Minimal Semon evidence viewer | Was a necessary companion to option 1. |
| Larger copy / fork | No evidence it lowers total cost. Not pursued. |

The proposed follow-up was a synthetic link-and-evidence spike instead of the Storybook comparison. That spike is superseded as the next step. Its link criteria still apply if an "Open in T3" link is later proposed:

- A mapped link resolves to the correct environment and thread. Ambiguous or missing mappings show as unavailable and are never guessed. Do not assume one native session names exactly one app thread.
- Semon evidence remains usable without T3. Stale or inexact anchors refuse rather than fall back to an ordinal, timestamp or similar text.
- The URL contains no body text, source path or credential. Opening it causes no import, provider start, resume or other write, and existing authentication still applies.
- Test cases include duplicate native IDs on two machines, an unavailable T3 environment, a stale Semon generation, an imported history older than 30 days, and a tool-only evidence target.

[sidebar-doc]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/docs/user/thread-sidebar.md
[sidebar]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/components/Sidebar.tsx
[timeline]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/components/chat/MessagesTimeline.tsx
[timeline-logic]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/components/chat/MessagesTimeline.logic.ts
[work-entry-logic]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/components/chat/TimelineWorkEntryRow.logic.ts
[contracts]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/packages/contracts/src/orchestrationV2.ts
[links]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/packages/shared/src/threadLinks.ts
[citation]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/packages/contracts/src/assistantCitations.ts
[citation-nav]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/lib/assistantCitationNavigation.ts
[scanner]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/server/src/project/AgentSessionScanner.ts
[import-schema]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/packages/contracts/src/agentSessions.ts
[importer]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/server/src/project/AgentSessionImporter.ts
[architecture]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/docs/internals/overview.md
[remote]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/docs/internals/remote.md
[web-package]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/package.json
[sidebar-primitive]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/apps/web/src/components/ui/sidebar.tsx
[readme]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/README.md
[license]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/LICENSE
[license-doc]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/docs/internals/open-source-licenses.md
[license-config]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/third-party-licenses.config.json
[auth]: https://github.com/pingdotgg/t3code/blob/c6ca6056309904f89b90b6055d797973be0a0939/docs/internals/environment-auth.md
