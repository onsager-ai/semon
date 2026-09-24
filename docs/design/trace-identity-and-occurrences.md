---
depends_on: [carrier-neutral-trace-storage.md]
identity_impact: closes the trace-granularity question without changing what identity ranges over
---

# Trace identity and the occurrence index

**A trace's identity stays exactly what it is: the content hash of its carrier-neutral semantic core. The log is carried by a new ordered region — occurrences — that records, for each time a trace was seen, where and when it was seen. The same sentence spoken in two sessions is one trace with two occurrences.**

Decided 2026-09-20. This closes the **OPEN — Trace granularity** row in [`carrier-neutral-trace-storage.md`](carrier-neutral-trace-storage.md), and it closes it with an argument about *what identity ranges over* rather than with a granularity number.

## The problem this answers

*(measured — see [#8](https://github.com/onsager-ai/semon/issues/8))*

The canonical document is `{"kind": …, "content": …}` and identity is the hash of exactly that. Nothing in the canonical region carries time, session, repository, or order, so the log is not merely unrendered — it is not derivable. Worse, identity over bare text merges genuinely distinct work: a short repeated instruction issued against two different pieces of work is one trace, and the canonical region retains nothing that could tell the two apart. Short repeated imperatives are the normal shape of directing an agent, so this is the common case, not an edge.

Three candidate answers were on the table, with costs stated. Turn-scoped and session-scoped documents both fix the log by making the document bigger — and both fix it by pulling session, time and repository *into* the hashed document, because that is what makes two otherwise-identical turns distinct. That is the move this decision rejects.

## The decision

Two *storage* regions become three, and the new one is ordered. A word of care about the count: [`carrier-neutral-trace-storage.md`](carrier-neutral-trace-storage.md) also says "three regions", but it means three classes of *field* within a trace — semantic core, carrier envelope, capability references — which is a separate axis and a separate unfinished job ([#13](https://github.com/onsager-ai/semon/issues/13)). The regions below are tables.

**`traces` — unchanged.** Content-addressed, carrier-neutral, `{kind, content}` as today. Identity remains the SHA-256 of the RFC 8785 canonical bytes of the semantic core.

**`occurrences` — new.** One row per time a trace was observed, carrying `(trace_id, session, sequence, timestamp, repo)`. Ordered. Not part of identity.

**`raw_carrier_records` — forensic source lines.** Schema v5 keeps one raw row per source line and links it to every projected trace through `raw_record_traces`; the forensic region is still reachable only through its own explicit read.

A log renders by joining occurrences to traces in occurrence order:

```
semon log --repo tolmap --day 2026-09-20
  → occurrences filtered by repo and day, in sequence order
  → joined to traces for content
```

Two occurrences of one trace are two lines in the log. They are not a duplicate, and they are not a collision. They are the accurate record that the same thing was said twice.

## Why this is not a retreat from carrier-neutrality

**This section is the one a later reader is most likely to get wrong, so it is stated plainly.**

Session identifiers, timestamps, repository attribution and ordering are exactly the fields [`carrier-neutral-trace-storage.md`](carrier-neutral-trace-storage.md) classifies as **carrier envelope** — excluded from the hash, retained for provenance. Someone reading the occurrence table later, without this document, will see those fields persisted in a first-class ordered region and conclude that the carrier-neutrality principle was quietly abandoned, and delete the table to restore it.

It was not abandoned. The principle says these facts must not participate in *identity*. It has never said they must not be *recorded*. Recording them in a region that identity does not range over is what lets the semantic core stay genuinely carrier-neutral under real use: without somewhere to put provenance, the pressure to make traces distinguishable pushes provenance into the hashed document, and carrier-neutrality dies by that route rather than by this one.

**The occurrence region is not a violation of the principle. It is the thing that makes the principle survivable.** Do not delete it to tidy the model.

The same reasoning preserves a property that was the point of content-addressing in the first place: `ship` idempotence stays a property of the content hash. An endpoint upserts by `X-Semon-Content-Hash` and repeated replication remains a no-op, because nothing session-specific has entered the hashed document.

## What this decides about granularity

The OPEN row asked for a unit: session, task, or decision. The answer is that the question was mis-shaped.

Granularity of the *semantic document* and granularity of the *log* are different questions, and conflating them is what made the original one unanswerable. A trace is as small as the smallest unit of transferable work content — today a message, and that may change as the projection widens. The log's unit is the occurrence, and occurrences are as fine as the carrier's own ordering allows. Neither constrains the other.

So the row closes with: **identity does not range over session, time, repository or order. Those are occurrence facts.**

## Consequences, and what each one resolves to

Three consequences follow from putting provenance in a second region. Two resolve now, on argument and evidence. The third resolves in part now — the part that a later phase would otherwise bake in — and defers only its semantics.

### Occurrences survive forensic deletion — decided

*(derived — the argument is retention)*

A raw carrier record carries complete source-line bytes and provenance. One line may project several occurrences, so the regions must be persisted independently rather than deriving occurrences from raw at read time.

The first two are foreclosed by the need to delete forensic data at all. Under either of them, purging forensics destroys the log: the user's history evaporates as a side effect of a privacy control. **That is the worst available coupling, because it makes doing the right thing cost the user their history, and so it makes the right thing not get done.**

*Amended 2026-09-21.* As first written, this section expected P4 to bound retention — "very likely no, not indefinitely". P4 decided the opposite: [`forensic-retention-and-exposure.md`](forensic-retention-and-exposure.md) retains the forensic region in full and indefinitely. The conclusion survives the changed premise, because the argument never depended on *automatic* deletion. It depends on deletion being possible at all — an operator removing a credential that landed in the store — and #20 made that a supported operation. Independent persistence is what lets that deletion leave the log intact.

So: **occurrences are written alongside raw records, persisted independently, and must survive raw deletion intact.**

This is testable, and should be tested rather than asserted: **the log must render correctly on a store whose `raw_carrier_records` region is empty.** Stating it now matters because the derived version is less code, and P1 would otherwise reach for it for that reason alone.

**The test must assert correct rendering, not a zero exit code.** A `semon log` that silently yields fewer lines when raw is empty passes an exit-code check and fails the actual claim — the same failure to discriminate, one level down. Pin it to a known store: the same fixture rendered with and without the raw region, and the two outputs identical byte for byte.

### How raw records link to occurrences — decided

*(measured, 2026-09-21; resolves the question this section's first draft left open)*

Independent persistence settled that the two regions do not derive from each other. It left open how they relate — and the first deletion command showed that "not at all" was wrong.

Raw records were linked only to `trace_id`. But a trace is per *content* and a raw record is per *capture*, and content recurs: across one real week of 42 sessions, 152 traces appear in more than one session, and the unparsed-command trace appears in all 42. Selecting raw records by trace therefore reached the wrong rows in both directions. Measured on that week:

- deleting one session's forensic data would have removed **2,386 rows from 41 other sessions** — 4.5x its own 672;
- deleting everything before a cutoff would have left **52% of the targeted rows in place** (2,186 of 4,222), because their content recurred after the cutoff — while reporting success.

The read path in `semon forensic --session/--day` had the same flaw, over-reading other sessions. It went unnoticed there because extra rows on a read do no damage; on an irreversible delete the same selection destroys data.

**The provenance key:** each raw record carries its own `(carrier, session, sequence)` and timestamp. Schema v3 introduced the key; schema v5 stores the source line at its line sequence while occurrences retain per-block sequences.

- `raw_record_traces` links each raw line to every trace it projects. Occurrences still carry nothing that points at raw, so the log survives forensic deletion.
- `TraceStore::capture_line` receives the line key and all projected blocks in one transaction. Claude uses `sequence_for(line, 0)` for the raw row and preserves each block's own occurrence sequence; Codex uses the line ordinal for both.
- Rows written before v3 have no session or sequence and cannot be backfilled, since `trace_id` alone cannot determine which occurrence they belonged to. Session and time selections report these rows rather than silently skipping them.
- Forgetting by trace deletes the entire raw line containing that trace, including bytes for other traces projected from it. Their canonical traces and occurrences remain, and `semon log` continues to render.

Re-measured on the same week after the change: one session's deletion removes exactly its 672 rows with **zero collateral**, and the cutoff deletion removes all 4,222 with **zero survivors**.

### Inferred occurrence facts record their basis — decided

*(derived)*

Session, repository and timestamp are derived inside `normalize_record`, not given. Repository in particular is inferred from a git remote URL, falling back to a working-directory basename, with an explicit `--repo` / `SEMON_REPO` override taking precedence over both.

Under the old model those inferences were discarded. Now they are persisted and user-visible, which means a wrong attribution stops being invisible and starts being a mystery. **A guard ships with its reason; so does a guess.**

So each occurrence records the basis alongside the value:

```
repo_source: git-remote | cwd-basename | explicit-override | none
```

Four values, not two: the override is a real basis that exists in the adapter today, and an attribution that came from a flag is a different kind of fact from one that came from a guess. This also makes the weak inference countable — "how many occurrences in this store rest on a basename" is a query, not an anecdote.

### Sequence: scope and edge decided, branch semantics deferred

*(derived, with the carrier shapes measured)*

Codex session JSONL is a flat append-only stream, so a line ordinal fits directly. Claude Code transcripts are a DAG. Measured across all 283 local Claude Code transcripts — 257,039 records, 94 sessions: 188,196 records carry a `parentUuid` and 351 do not, so sessions have **many roots, not one**, because subagent sidechains start their own. 188 distinct agent ids appear.

Three things are settled now, because P2 would otherwise bake in their opposites:

**1. `sequence` is adapter-supplied and comparable only within one session of one carrier.** Never across carriers, never globally. Without this written down, `semon log` will treat it as globally meaningful the first time it merges two sources into one view, and the assumption then lives in the rendering rather than in a column, where it is far more expensive to remove.

**2. The edge is load-bearing; the ordinal is derived from it.** The two are not interchangeable, and the asymmetry decides the schema: a total order **cannot** reconstruct a DAG, whereas a DAG plus a deterministic tie-break **can** always produce a total order. So the occurrence row carries a nullable parent, and this is a **P2 schema question, not a P3 semantics one**. An additive migration is cheap; a `semon log` whose model assumes linearity is not, and that is the thing that would need migrating.

This is not Claude-specific special-casing. In a flat carrier the parent is simply the previous occurrence in the session, so one nullable column serves both shapes.

**3. The edge is between occurrences, not between traces.** This is easy to get wrong and expensive to correct later. A trace occurs many times by construction — that is the entire point of the occurrence region — so a parent column referencing `trace_id` would be ambiguous exactly when the log is most interesting. Occurrences therefore need their own row identity, and the parent references that.

What is **not** settled, and correctly waits for the Claude adapter (P3): what `sequence` *means* across a branch — whether a branched session yields one ordering with a tie-break rule, several, or an ordering per agent id. That is adapter semantics and needs the adapter in front of it.

## Carrier shapes, measured

*(measured 2026-09-20 — full census, superseding the sampled figures this document was first drafted against)*

Recorded here because P3 will build normalizer branches on these, and a wrong branch is expensive after the fact and cheap before it.

Across all 283 Claude Code transcripts on the author's machine (257,039 records; 9 lines unparseable, presumably partial writes):

- **Tool results are `user` records with list content 99.46% of the time, not always.** Exact block-type sets across 44,149 list-content `user` records: `{tool_result}` 43,912, `{text}` 224, `{image, text}` 13. A normalizer branch reading "list content implies tool result" is wrong on roughly one record in 190. Of those 237 exceptions, 194 are `isMeta` and 43 are not, so the rate at which it misfiles *human* prose is nearer 43 in 44,149 — smaller than it first looks, and still not zero.
- **Genuinely mixed records exist and are rare.** No `user` record mixes `tool_result` with anything. The mixed ones are 13 `{image, text}` user records and 3 assistant records (2 `{thinking, tool_use}`, 1 `{text, thinking, tool_use}`) — **16 in 257,039**. Rare, but a record-level classifier has no correct answer for them at all.
- **String content does not imply a human prompt.** Of 4,842 string-content `user` records, 2,198 are `isMeta`, 297 are harness injections, and 267 are sidechain records — a subagent's task prompt, not the human's.
- **Thinking outweighs prose.** 23,770 `thinking` blocks against 17,110 assistant `text` blocks. A Codex-shaped kind filter discards the larger half.
- **Record types with no Codex referent**, by volume: `attachment` (50,532), `queue-operation`, `last-prompt`, `bridge-session`, `mode`, `pr-link`, `ai-title`, `permission-mode` (4,968), `frame-link`, `file-history-snapshot`/`-delta`, `agent-name`, `cost-state`.

### Classification is per block, not per record — decided

*(derived, from the census below)*

The 0.54% above is not a rate to handle with a branch. It is a sign that the record is the wrong unit to classify, and two facts settle it.

**Mixed records have no correct record-level answer.** 16 records in 257,039 carry blocks of different kinds — 13 `{image, text}`, 3 assistant records mixing `thinking`, `text` and `tool_use`. A record-level classifier must pick one kind and discard the rest. Rare is not the same as handleable.

**More decisively, both carriers are already block-shaped.** Claude Code's `message.content` is a list of typed blocks. Codex's `UserMessage` and `AgentMessage` carry `content: [{type, text}]`. Classifying per record means inventing a record-level kind that neither carrier has, then deriving it by a rule — "first block wins", "any `tool_result` wins". That derived rule is the thing that breaks when the ratio drifts, and it breaks silently. Per-block classification is not a fix for a percentage; it is declining to invent a level of structure the carriers do not have.

So: **one record may contribute several blocks, each classified by its own type, and the intent/outcome projection is per block.** Written that way the exceptions are the ordinary path rather than a branch, which is the only version that stays correct as the ratio moves.

This is schema-shaped, not an adapter detail: it decides what an occurrence is an occurrence *of*.

### Occurrences need their own uniqueness key — decided

*(derived, with the re-read path measured)*

Trace idempotence does not extend to occurrences, and the gap is a correctness hole rather than a refinement.

Re-capture is a no-op today because the content hash collides and the canonical row already exists — measured: a second pass over the same sessions consumed 0 records. But an occurrence is *one row per time a trace was seen*, so "already recorded" is **not expressible by content at all**. Two occurrences of one trace are the region's entire purpose; they cannot also be the signal for a duplicate write.

**The failing path is in the code, not hypothetical.** `process_file` resets the cursor whenever the file has shrunk below the saved offset:

```rust
if saved.offset > size {
    saved = state::FileCursor::default();
}
```

`FileCursor` carries a byte `offset` and no inode, size-at-save, or content digest, so a truncation restarts the file at zero and every record is re-emitted. Traces dedupe silently, as designed. Occurrences would duplicate — and the log would grow phantom repetitions of real work, **indistinguishable in the rendered output from the user having genuinely said the same thing twice.** That is precisely the distinction this region exists to draw, failing in the direction that looks like data rather than like a bug.

So the region carries a natural key and the write is an upsert on it:

```
unique (carrier, session, sequence)
```

**Two conditions make that key actually work, and both are consequences of decisions taken above rather than free choices.**

**`sequence` must be deterministically derived from the source, never assigned at write time.** An autoincrement produces fresh values on every re-capture, the upsert never matches, and the key silently buys nothing. Record order is byte order within the file and block order is array order within the record — both stable — so a re-read regenerates identical values. This is what makes the upsert idempotent, and it is the property to test.

**`sequence` enumerates occurrences, not records.** Since classification is per block, one record contributing three blocks contributes three consecutive sequence values. Under a per-record ordinal the key would not be unique, so the per-block decision and this key have to be stated together or the key is wrong.

The test case is the truncation-restart path itself: capture a file, truncate it, capture again — and assert that **the occurrence rows are identical, field for field, not that their count is unchanged.** A count assertion catches only the failure where the upsert misses entirely; it passes unchanged while `sequence`, `repo_source` or `authored_by` are recomputed differently on the second pass, which is the failure this key exists to prevent. Name the representation the assertion holds at, or the control cannot fail on the input it was written for. Worth settling while the schema is still text — after P1 this is a migration, and after P2 it is a migration plus a backfill over rows nobody can tell apart.

### Authorship is a stored fact, not a filter — decided

*(derived)*

"Human prompt" is currently definable only as a chain of exclusions: not sidechain, not meta, not a known injection prefix. Such a filter **silently widens every time the harness invents a new kind of non-human string**, and the census shows it inventing them steadily — `frame-link`, `agent-name`, `cost-state`, `continued-in` and `artifact-autoreact-ledger` are all present now and were not in the shapes this document was first drafted against. `isMeta` alone accounts for 2,198 of 4,842 string-content user records.

So authorship is derived by the adapter and **stored on the occurrence**:

```
authored_by: human | agent | harness | unknown
```

Two reasons it is a stored classification rather than a drop. A stored value can be audited and counted later; a drop leaves nothing behind to audit. And when the harness ships a shape the adapter has not seen, the record becomes a visible `unknown` rather than silently landing in `human` — which is the failure that a chain of exclusions produces by construction.

**Authorship belongs on the occurrence, not on the trace, and the reason is not self-evident.** Harness boilerplate *feels* like a property of the text, so hoisting `authored_by` onto the trace to avoid repeating it per row is the tempting move. It would mean the same string is permanently harness text even when a human types it — which is the #10 bug re-entering through the very axis this separates. Authorship is provenance, and provenance lives with the occurrence. It also gives the honest denominator: a boilerplate block injected into 400 sessions is 400 facts about the store, not one, so "how much of this store is harness text" counts occurrences.

`authored_by` is **not** the same axis as the semantic core's `kind`. `kind` says what the content is — a goal or a result. `authored_by` says who produced it. A harness injection has a `kind` too; that is precisely how four of nineteen traces became work intent and outcome in the first place. Collapsing the two axes would reintroduce that bug under a new name.

### The occurrence row carries the agent — decided

*(derived)*

188 distinct agent ids appear across 94 sessions. Sidechain *structure* and sidechain *attribution* are different queries, and only the first is served by the parent edge.

"What did I do in repo X on day Y, **excluding subagents**" is the P2 gate almost verbatim, and answering it by walking each occurrence to its sidechain root is a recursive join for the single most common question the log will be asked. So `agent` is stored alongside the parent, nullable, on the same argument as the parent column: in a flat carrier it is null and nothing is special-cased.

## Effect on the P2 read-surface gate

The gate in [#11](https://github.com/onsager-ai/semon/issues/11) is: a human can answer *"what did I do in repo X on day Y"* from the CLI, without opening SQLite, **and without the command touching `raw_carrier_records`**.

This decision makes that gate **sharper, not easier**. Before it, the clause was arguably unreasonable — the canonical region genuinely could not express the answer, so a reader hitting the wall could fairly conclude the gate was the problem. With occurrences, the answer is expressible from the non-forensic regions in principle.

So failing the gate is now evidence that P0 or P1 was implemented wrong, not evidence that the gate is unreasonable. **That is the reading to apply when the gate is inconvenient.** The clause stays.

Two of the decisions above land on P2 as schema, not as rendering, and are the reason they are settled here rather than at P3: the occurrence row carries a **nullable parent referencing another occurrence**, and `sequence` is **scoped to one session of one carrier**. A `semon log` built without those assumes a global linear order, and that assumption then lives in the rendering — which is what would have to be migrated, rather than a column.

## Not decided here

No implementation is authorised by this document. The decisions above constrain the occurrence region's schema; the schema itself, its write path, and the migration are code, and they go through the plan→implement checkpoint separately. What `sequence` means across a branch stays open for P3. Capture remains uninstalled: the systemd installer is not to be run and `~/.local/share/semon` is not to be created until the exposure policy of P4 exists, because that installer creates a credential-bearing artifact on first run.
