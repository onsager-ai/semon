# Proven edge builder

Viewer transcript and Trace should link sessions only when the warehouse can
prove the exact relationship. #741, #749 and #754 spread that decision across
readers, letting one path bypass checks another had added. Keep #754's
sender-side cards with “not linked yet” targets until the shared builder can
certify them.

## Proof contract

A proven edge is a directed relay, launch or handoff between two distinct
canonical session owners. It carries the exact source event and target session
or entry address, with the namespace, read scope and generation that qualify
those addresses. A spawn return is a separate edge from the child to the parent
result with its own evidence. A launch alone does not prove a return. Keep
recorded actions visible when their links are unavailable, with a reason such
as missing receipt, ambiguous ownership or unsupported native evidence.

Build from `catalog_warehouse::selected::ReadView`. Use its pinned SQLite
transaction and current Index checks, including `storage::sqlite::qualify` for
the supported storage stamp and matching bound namespace (or an unbound store).
Scope all evidence to the requested namespace and Index incarnation even for an
unbound store. Qualify both owners, carrier revisions, consumed prefixes and
address occurrences in that transaction. Use the committed Current publication
and its source ledgers, or the exact Retained identity and certified peer
prefixes. Retained prefixes bound what can be linked. The namespace-wide
ownership check may see additional Current or certified Retained facts in the
same transaction. On navigation, recheck the issued binding and proof in the new
request's snapshot. Reuse authorized source reads to validate addresses and
render previews. The builder works on metadata. Body text stays out of analytics
and the Hub.

- **Relay.** Require one physical send and its exact, later successful typed
  result, one correlation owned by that send, and one canonical native receipt
  that resolves to a receiving turn. Check the recorded destination against the
  receipt owner's qualified native identity or exact Catalog key. For `main`,
  require the sender's unique direct native parent. Read native parent evidence
  separately from the general Catalog parent offer, which can select a marker,
  process ancestry or an environment value.
- **Launch.** Require one child with a direct native parent and one exact parent
  launch call. For Claude require the child's revision-qualified sidecar or
  certified receipt to name its storage parent and `Agent`/`Task` call. Match
  that call on the actual parent carrier. A marker cannot supply the binding.
  For Codex require native parent identity and the warehouse's unique spawn
  placement for the exact call. For a retained prefix, apply the same placement
  rule to certified native facts instead of borrowing a Current placement for a
  different revision. Check the reverse claim on the physical call for both
  harnesses. Two distinct children claiming it revoke both links.
- **Handoff and return.** Require one native correlation or explicit source
  event reference shared by the initiating event and receiving entry, with both
  events mapped to their native owners. A relay handoff uses the proven receipt
  and its receiving turn. A synchronous spawn return needs the proven launch,
  a result bound to that physical call, and native evidence identifying that
  child's exact completion. Keep local turn and result rendering available
  independently. Text, timing, prompts and `pair_up` cannot supply an edge or a
  cross-session completion.

Probe contenders by correlation or launch-call identity across the qualified
namespace before applying the request's witness, page or Trace limits. Test
uniqueness on physical events and canonical owners. Collapse only copies whose
identity the warehouse already proves. A competing owner, conflicting claim,
incomplete probe or missing binding suppresses the edge. Recompute on each
snapshot so a newly admitted conflict, revoked placement or source replacement
withdraws a previously visible link. For example, a second sender outside a
retained witness still revokes the first sender's link inside it.

## One read path

Put the builder in `catalog_warehouse`, beside `selected` and `parents`. Reuse
warehouse lineage, spawn placement and indexed fact probes. Give its proven
edge type a private constructor. Current and Retained readers supply candidate
facts and receipt bounds to the same rules. Move ownership and lineage decisions
out of `retained_relay.sql`, `retained.rs`, `selected.rs` and `parent_relay.rs`.
Keep the general parent offer for Catalog context. `http.rs` and `trace.rs`
consume proven edges for cards, previews and traversal. Trace applies its
traversal limits after proof and uses the builder for spawn returns as well.
`relationships.ts` and `tracePresentation.ts` render server-issued targets or
unlinked evidence. They never turn a candidate identity into a link.

Compute edges at read time from existing facts and receipts. This design needs
no `CATALOG_VERSION`, Index schema/stamp or parser `CACHE_VERSION` bump. Missing
or insufficient retained facts yield unlinked evidence. If implementation finds
it must persist a new fact or change a parser projection, version that owning
contract explicitly and update its producers, readers and fixtures together.
Use one Viewer wire contract and regenerate the bundle with the cutover. Keep
native custody intact and omit compatibility readers and migration layers.

## Verification and size

Use CI fixtures for valid Current and Retained relay and launch edges, native
handoffs and returns where supported, and parity across transcript and Trace.
Add a regression for each bypass above, including a second sender outside the
witness, a process/environment parent, a marker-only retained launch, two Claude
children claiming one call, and a return that previously matched via `pair_up`.
Exercise duplicate receipts, proven copied prefixes, wrong stamp/namespace,
revision mismatch, and revocation between snapshots. Check that unlinked cards
still render and offer no navigation. Check that exact links open the intended
entry and that proof probes stay keyed as unrelated sessions grow.

Estimate **two implementation PRs and 20–28 agent hours**. Add the builder and
proof fixtures without enabling links (12–16 hours). Then switch Current,
Retained, Trace and the Viewer together, remove bypasses and verify endpoint
parity (8–12 hours). The estimate assumes the existing facts can prove the
supported cases, with insufficient evidence left unlinked.
