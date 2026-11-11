# Agent Session Intelligence (ASI): a generic methodology

Version 2.1, 2026-11-06. This method doesn't depend on any one harness, model, repo or product. A worked example (2026-11-04 baseline) and join and cost coverage (2026-11-05) are kept as private research records; the aggregate figures quoted below come from them.

Moved from semon-hub `docs/asi-methodology.md` on 2026-11-08; this copy is now canonical. Product scope and sequencing live in [the ASI direction](asi-direction.md).

Changes in 2.1, all additions to v2 labels (no pilot label needs redoing): steps carry a context band and a cost (§3.1.1, §5); context carry cost is reported beside the levers rather than as waste (§3.1.3); role may be `unknown` (§3.2); a session's outcome may be `unlinked`, and links record which evidence produced them (§1.1, §3.2); a list-price shadow cost is allowed when billing is unavailable (§5).

## 0. Purpose and principles

ASI answers three questions about any body of agent work, and then a fourth:

1. **Cost.** What did the work consume?
2. **Benefit.** What did it deliver that someone accepted and valued?
3. **Quality.** How good was what it delivered?
4. **Cause.** Where does cost leak away from benefit, and which change would improve the ratio?

Principles:
- **Measure outcomes, not activity.** Tokens, steps and commits are inputs or intermediate products.
- **Fill the gaps with meaning, not guesses.** Logs record what happened. Intent, attribution, value and waste are semantic facts. Classification recovers them, and humans calibrate the classification.
- **Every number carries its coverage, its uncertainty and its evidence grade.**
- **Claim causation only from interventions.**

## 1. Data model

```
Event ─▶ Step ─▶ Segment ─▶ Turn ─▶ Session ─▶ Lane ─▶ Work item
                                       └──── Link ────▶ Change ─▶ Outcome
```

| Entity | Definition | Source |
|---|---|---|
| **Event** | One raw record: a model call, tool execution, message, compaction or abort. Immutable. | Harness logs, or OpenTelemetry GenAI spans |
| **Step** | One agent decision: a model call, the tool calls it requested and their results. Parallel tool calls from one model call belong to one step. A spawn is a step in the parent, linked to the child session. | Derived, deterministic |
| **Segment** | A contiguous run of steps that pursue one goal. It may cross turns when the goal continues ("continue", "go"). | Rule candidates plus semantic selection (§4.1) |
| **Turn** | From one external input (user, Manager or peer message) until the agent goes idle | Harness |
| **Session** | One agent context from start to end | Harness |
| **Lane** | A root session plus every session it spawned or delegated to, transitively | Lineage metadata, or semantic linking |
| **Work item** | The unit of intent: an issue, a task, a request, a question | Tracker, or semantic linking |
| **Change** | A unit of accepted artifact: a merged PR, a deployed config, a published document, a delivered answer | Artifact system |
| **Outcome** | What became of a change: CI on merge, later fix, revert, survival. Null until its window closes. | Deterministic plus semantic (§3.3) |

The analogy is a trip in a map product: a session is the trip, segments are the legs and steps are the manoeuvres. Unlike navigation, legs are reconstructed after the fact, as a location history infers trips and visits from raw pings, and there is no computable optimal route. ASI therefore measures detours it can observe (waiting, U-turns, dead ends), not distance from an ideal path.

**Why steps.** Every observability product has this level, usually as the parent span of one agent iteration: an OpenTelemetry span around `chat` and `execute_tool` spans, a LangSmith chain run, a Langfuse span, an OpenInference CHAIN or AGENT span, a smolagents `ActionStep`, an OpenHands action-observation pair. ASI needs it because it is:
- **harness-neutral**: each harness logs calls and results differently;
- **the unit a decision is attributed to**: a tool result only makes sense with the call that asked for it;
- **the unit for deterministic labels**: activity, success and waste are properties of an action and its result;
- **the address for evidence and boundaries**: labels cite step IDs, and segment edges fall between steps;
- **the unit of cost**: model tokens plus tool time.

None of these products has segments. Segments are what ASI adds.

### 1.1 Storage schema

```
COLLECTION (raw, immutable)
source       (id, harness, machine, archive_coverage_from, archive_coverage_to)
session      (id, source_id, harness, model, role[manager|worker|solo|unknown], parent_session_id,
              parent_method[native|workspace|none], lane_id, repo, started_at, ended_at, status)
turn         (id, session_id, idx, started_at, ended_at, initiator[user|manager|peer|self])
event        (id, turn_id, ts, kind, tool, status, input_tokens, cached_tokens, output_tokens,
              latency_ms, context_tokens, payload_ref)

DERIVED (deterministic, re-computable)
step         (id, session_id, turn_id, idx, started_at, ended_at, event_ids[], tool, target,
              activity, ok, exit_code, tokens_by_class, context_tokens, context_band, model_ms,
              tool_ms, usd, price_version, waste_kind|null, trigger|null)
segment      (id, session_id, step_from, step_to, goal_text, boundary_rules[],
              activity_mix, primary_activity, cost, waste, segmenter_version)
cost         (target_type, target_id, tokens_by_class, model_s, wall_s, usd, price_version)

SEMANTIC (many labellers per target)
label        (id, target_type, target_id, field, value, mechanism_text, evidence_step_ids[],
              labeler[rule:x|model:y|human:z], taxonomy_version, confidence, created_at)
adjudication (id, label_ids[], final_value, adjudicator, reason)
lever        (target_type, target_id, lever, mapping_version)         -- derived (§3.1)

WORK AND BENEFIT
lane         (id, title, root_session_id, work_item_id)
work_item    (id, tracker, ref, value_class, size, priority, opened_at, closed_at)
change       (id, repo, kind, ref, opened_at, merged_at, reverted_at)
link         (from_type[session|segment], from_id, change_id,
              method[trailer|commit|push|branch|mention|semantic], evidence[produced|mentioned], confidence)
outcome      (change_id, ci_on_merge, later_fix_change_id, escaped_defect, survival_30d,
              window_closed_at)                                       -- null = censored

EXPERIMENTS
price        (model, valid_from, input, cached, output)
intervention (id, lever, hypothesis, primary_metric, guardrails, preregistered_at)
assignment   (intervention_id, unit_type, unit_id, arm, assigned_at)
```

Durable implementation notes (#560): every durable table has an author-assigned
surrogate `id` and an internal writing `revision`. Composite identities remain
unique: link `(from_type, from_id, change_id, method)`, outcome `change_id`, and
assignment `(intervention_id, unit_type, unit_id)`. Durable UIDs are `{table}/{id}`.
The label's `target_id` is stored as `target_uid`. Adjudication additionally stores
required `target_type`, `target_uid`, `field`, `taxonomy_version` and `created_at`;
its nonempty label list must all exist and match those four target/taxonomy fields.
It exports one human assertion to `fact_label`, with ID `adjudication/{id}`, final
value, `human:<adjudicator>`, null confidence, its timestamp and writing revision.
Labels export IDs `label/{id}`. Arrays are JSON text; times are UTC milliseconds.

The complete column/class declaration lives in `semon-asi::durable::TABLES`.
Body columns are `label.mechanism_text`, `adjudication.reason`, `lane.title`,
`intervention.hypothesis` and `intervention.guardrails`; they remain author-local
and are read only through focused view. Intervention `lever` and `primary_metric`
are categorical (not exported by #560). Identity/reference/array/time columns are
internal; other columns are categorical as declared in that table. See
[Durable state](asi-store.md#durable-state) for export and ownership details.

Rules:
1. **Raw events are never modified.** Everything derived can be recomputed when segmentation, rules or prices change.
2. **Labels are rows, not columns.** Rule, model and human labels for the same target sit side by side, so agreement and calibration are queries.
3. **Links carry their method, evidence and confidence.** Report cost per change per link method. Trailer links are grade A. A commit printed or a branch pushed in the session is `produced` evidence, also grade A, though incomplete. A URL or SHA mention is only `mentioned` evidence: reviews and monitoring mention changes too. Semantic links are grade B at best. Report link coverage beside every per-change figure.
4. **Outcomes stay null until their window closes.** Benefit metrics use closed windows only.
5. **Metrics are views**, not stored numbers.

Joins between these entities are the core difficulty. Prefer explicit keys (session and lane IDs embedded in artifacts). Where there are none, recover the join semantically (§3) and grade it accordingly.

## 2. Measurement layers

| Layer | What it produces | Method | Default grade |
|---|---|---|---|
| **L0 Deterministic** | Timings, tokens, context size, tool failures, artifact states, CI results, lineage | Parsers and APIs | A |
| **L1 Semantic** | Intent, joins, segment labels, outcomes, waste class, claim truth, change quality | Model classifiers on bounded evidence packets | C until calibrated, then B |
| **L2 Human adjudication** | Gold labels on disagreements plus a random calibration sample | Review UI | Reference |

The human reviews L1's disagreements and a calibration sample, never the raw event stream. Labelling individual events is the wrong level of detail: it is expensive, and its signal is mostly already in L0.

## 3. Semantic label schema

### 3.1 Step and segment labels

Label only what is observable. Ask no model to say what would have prevented a problem: that counterfactual is where judges and people disagree most. Map observations to levers by rule instead (§3.1.3).

#### 3.1.1 Step labels (deterministic)

**Activity**, first matching rule:

| Value | Rule |
|---|---|
| coordinate | Spawns, messages or steers another agent or session |
| wait | Sleeps, polls or blocks on something external without changing state |
| integrate | Changes version-control or PR state (commit, push, rebase, merge, PR create/merge) |
| verify | Runs checks or reads their results (tests, builds, lint, CI status and logs) |
| recover | A state change whose target matches a failure in the preceding steps |
| implement | Any other change to repository files |
| explore | Reads or searches without changing anything |
| setup | Changes the environment rather than the repository (install, auth, worktree, config) |
| report | Produces a message to the user or Manager |
| think | A model call with no tool call and no message |

**Waste kind** (zero or one per step): wait (including repeated checks with no change in between) · failed call · retry · redundant read (the same read repeated with no change in between) · context refill (re-reading after compaction what was read before it).

**Context band** (every step): the step's context size (uncached plus cached input) in bands, by default under 50k, 50–150k and over 150k tokens. **Cost** (every step): the tokens of the step's model call by class, priced (§5). A model call that requests several tool calls is one step, so nothing is split.

**Trigger** of a failure or waste step, from the preceding steps: tool error · empty result · test or CI failure · external error (rate limit, 5xx, auth) · compaction · polling · Worker result · instruction change · missing input · none visible.

#### 3.1.2 Segment labels (semantic, observable)

| Field | Values | Notes |
|---|---|---|
| Goal | free text, verb first, at most 12 words | for people and clustering, not a category |
| Outcome | achieved · partial · abandoned · failed · unknown | decided by ordered rules: primary goal only; failed if the last attempt fails unfixed; achieved by a success test per goal kind (change verified or integrated, facts used later, report sent, awaited result arrived, Worker started); partial only with a cited continuing segment; otherwise unknown |
| Rework | none · redo own · redo other's · revert, plus the reworked segment | rules propose from file and goal overlap |
| Dead end | yes · no · not applicable | explore segments whose findings are never used |
| Missing input | yes · no | the agent visibly hunts for information the brief should have given |
| Instruction change | yes · no · unknown | a message arrives and the work changes direction |
| Mechanism | free text, at most 12 words | the main waste or failure, concretely |

Every semantic label cites evidence step IDs, and carries its labeller, taxonomy version and confidence. Write each field as an ordered decision rule, not a definition: in the pilot, ordered rules raised agreement on outcome from κ 0.51 to 0.61 and on dead end from 0.67 to 0.94 (dead end had only 1–2 positives, so that κ is unstable). A field whose evidence the privacy projection hides (instruction change, missing input) cannot be rescued by rules; give it a privacy-safe signal or leave it out of the core. Activity mix, cost and waste on a segment are sums over its steps.

#### 3.1.3 Levers (derived by rule)

| Observed pattern | Lever |
|---|---|
| Context refill, or trigger is compaction | Context |
| Failed call or empty result from search or file tools | Tooling |
| External error, or a CI failure that passed on rerun | External |
| A Manager waiting or polling while a Worker or CI runs | Orchestration |
| Rework, missing input, or abandonment after an instruction change | Task specification |
| A test or CI failure after a segment marked achieved | Verification |
| A change delivered but not accepted after N days | Integration |
| Anything else | Unmapped: human review; its mechanism text feeds subtypes |

Levers cover avoidable waste only. The cost of carrying a large context on every call is structural, not waste, and it is often the largest cost: 48% of list-price cost in October 2026 was cache reads. Report it as **context carry cost** by context band, beside the levers and in the same units (hours and cost), so that context interventions have a metric. Only refill after compaction counts as Context waste.

The mapping is versioned like code. When a rule proves wrong, change it and re-derive; no labels need to be redone.

#### 3.1.4 Fixed core, dynamic subtypes

The fields and values above are the fixed core: they change rarely, with a version bump, a re-labelled calibration set and a note in the evidence record. Never change them during a running intervention.

Subtypes under each lever are dynamic. They are promoted from mechanism texts:
1. Cluster the mechanisms periodically, for example monthly. A model proposes clusters; a reviewer accepts or rejects them.
2. Promote a cluster to a subtype only if it recurs across sessions, implies a different intervention from existing subtypes, and two labellers agree on its examples.
3. Review the taxonomy when "unmapped" exceeds about 10–15%, a subtype grows fast, or agreement for a label drops.
4. Every label stores the taxonomy version it was made under.

**Why not an error taxonomy.** Benchmark taxonomies such as TRAIL's classify cognitive failure types (hallucination, misinterpretation, goal deviation). They are hard to agree on, because one failure has a symptom and a cause, and they do not name a fix. Use them to benchmark judges, not to steer work.

### 3.2 Session and lane labels

- **Work item(s) served**, with a weight for each when a session serves more than one.
- **Work type:** feature · fix · integration · infrastructure/CI · investigation · review · orchestration · maintenance.
- **Role:** root/manager · worker · reviewer · unknown. Use native lineage first. A short, non-interactive session started in another lane's workspace (a headless review or probe launched from another harness) is a worker whose parent is found by workspace and time (`parent_method = workspace`, grade B). If neither applies, the role is unknown, never root by default.
- **Outcome:** delivered (with a link) · contributed (to another session's delivery) · inventory · knowledge (a finding someone later used) · superseded · abandoned · harmful (introduced a defect) · unlinked. Use `unlinked` when the session changed artifacts but no `produced` link connects it to a change. Never infer inventory or abandonment from a missing link.
- **Claim truth:** compare each claim in the final report with L0 evidence. Labels: verified · unverifiable · overclaim · false.

### 3.3 Change labels

- **Intent match:** yes, partial or no, against the work item's description.
- **Test adequacy:** exercises the change, superficial, or none.
- **Escaped defect:** a later change fixes a defect *introduced by this one*. It counts only if all of these hold:
  - the faulty code was added by this change;
  - the later fix is not a planned follow-up;
  - the later fix is not a design change;
  - the behaviour the fix needed existed when this change was accepted.
- **Verdict:** sound, minor issues, or defective, with a confidence.

### 3.4 Work-item labels (value)

- **Size:** S, M or L.
- **Priority:** must, should or could.
- **Value class:** user-facing, enabling (infrastructure), risk reduction, or knowledge.

These are assigned once per item by its owner, or proposed by a model and confirmed by the owner. They are the only labels that need the owner rather than any reviewer.

## 4. Classification pipeline

1. **Evidence packets.** Each packet is a bounded, self-contained bundle of evidence for one item:
   - the description;
   - relevant excerpts;
   - linked artifacts;
   - what happened next.

   Apply caps by priority. Always include high-risk content, such as config, CI and security files, whole.
2. **Triage.** A cheap model labels everything. Tune it for recall.
3. **Review.** A stronger model labels everything triage flagged, plus a random sample, at least 10–15%, of what triage cleared. It runs blind to the triage verdict, and blind to any signal it will later be scored against (for example, CI results when the target is defects CI missed).
4. **Adjudication.** A human resolves the disagreements and labels the random calibration sample in the review UI. Where no human labeller is available, two strong models from different providers can stand in as reference labellers; their agreement bounds what can be calibrated, and the results stay grade C until humans check a sample. Each adjudicated decision is checked against evidence, for example "did this change add the line the fix changed?"
5. **Calibration.** Estimate each classifier's precision and recall per label from the adjudicated set, with intervals. A label set's results are grade B only once both its precision and recall intervals meet the targets you set.
6. **Diversity over escalation.** When two cheap judges perform alike but err differently, their union raises recall more than a stronger single judge does. Measure the overlap before choosing a cascade shape.
7. **Drift control.** Re-sample periodically. Re-calibrate whenever a model, prompt, harness or packet format changes.

Sample sizes:
- To estimate a rate to within ±5% at 95% confidence, you need about 385 items. ±10% needs about 97.
- To bound a miss rate below *p* when you observe zero misses, you need about 3/*p* cleared items. For example, 100 items for 3%.

### 4.1 Segmenting turns

Turns are the outer boundary. Inside a turn there are two levels:
- **Phases:** what kind of work is happening. These are deterministic, derived from runs of tool categories.
- **Goal segments:** which sub-goal the work serves. These are semantic.

Pipeline:
1. **Rule candidates (high recall).**
   - Hard splits: user or agent messages, spawns and handoffs, compactions, reports.
   - Signals: a commit or push, a PR milestone, a change of working directory or worktree, a shift in the files touched (low Jaccard overlap across ±6 steps), a build or test going from fail to pass, a gap of more than 5 minutes.
2. **Step digest.** One line per action: time, category, exit code, reasoning-summary titles, command verbs, file basenames, PR/issue references. Apply a privacy projection before anything leaves the machine: withhold message, commentary and argument text, and paths and IDs.
3. **LLM goal segmentation.** Feed overlapping windows of about 180 steps with the candidates marked. The model returns tiling segments with a goal each. Validate that the segments tile the window, and re-run any window that fails. Label outcomes and the other segment fields (§3.1.2) in a separate pass over whole turns, so each labeller sees what came next.
4. **Stitching.** Each window owns its interior; split the overlap at its midpoint.
5. **Calibration.** Humans mark split points and goals on sampled turns. Score with boundary F1 at ±3 steps, plus goal-label agreement.

Measure deterministic waste at step level (§3.1.1). Ask the model only for the semantic segment fields (§3.1.2). Segment-level waste tags over-attribute time.

**Meta-work trap.** When a session analyses other transcripts, its reasoning titles describe the material being analysed, not its own actions. Detect such sessions and segment them from actions only.

## 5. Cost ledger

```
cost(session) = Σ_steps Σ_token_class price(model, class) · tokens
      classes = uncached input, cached input, cache write, output, reasoning
cost(lane)    = Σ over the sessions in the lane, plus attempts that never produced a change
```

- Take prices from billing where a per-key usage export exists. Reconcile against billing daily. If the gap exceeds 5%, the cost figures for that period drop to grade C.
- Without billing, report a **list-price shadow cost**: tokens × a dated snapshot of the gateway's price list, with `price_version` stored. It is grade B at best: tokens are exact, the price is not what was billed. Never present it as spend. Calls whose model has no price are counted and reported, not dropped.
- Never report raw token totals as cost. Cache pricing can change the result by an order of magnitude.
- **Two clocks:**
  - **Machine-active time:** turns, including aborted ones.
  - **Human-attention time:** prompts, reviews, decisions.

  Idle wall time affects lead time, not spend.
- **Cost drivers** (diagnostic only):
  - context area (the sum of context size over steps);
  - spend by context band;
  - latency as a function of context;
  - compactions and their refill cost;
  - failed calls;
  - fan-out (sessions per lane);
  - serial manager time.

## 6. Benefit ledger

Report every rung, so it is visible where value leaks:

| Rung | Definition |
|---|---|
| B0 Output | Artifacts produced: commits, documents, answers |
| B1 Delivered | Accepted into the target system: merged, deployed, sent |
| B2 Accepted | B1, minus anything that broke the system's checks (excluding confirmed flakes), was reverted, or carried an escaped defect |
| B3 Useful | B2 that closes its work item's acceptance criteria, survives N days unchanged, and isn't superseded. Knowledge outcomes count here when a later session or decision used them |
| B4 Value | B3 weighted by the value labels (§3.4). If money is required, multiply by a declared conversion (for example, human-equivalent effort × rate), labelled as an assumption |

**Attributing benefit.** A lane's outcomes are credited to the lane. Within a lane, split credit by the segment labels' work-item weights. Sessions labelled "contributed" receive credit through the delivering session's lane.

**Censoring.** Judge an item only after its observation window has closed: for example, 14 days for escaped defects and 30 days for survival. Inventory is unfinished work, not failure. Report it separately, with its age.

## 7. Quality ledger

- **Deterministic signals:**
  - checks turned red by a change (separating persistent failures from flakes);
  - reverts;
  - later changes that explicitly reference and fix this one.
- **Semantic signals:** the change labels in §3.3, from the pipeline in §4.
- **Claim truth:** the share of final-report claims verified against evidence, by claim type.
- Deterministic and semantic signals catch different failures. Report both, and their overlap.

## 8. Bottleneck analysis

1. **Leakage funnel.** Conversion from B0 to B4, and cost spent at each rung that never reached the next.
2. **Waste decomposition.** Cost and time by waste kind and by lever (§3.1), by role and by context band.
3. **Critical path.** For each lane, the chain of segments that set its lead time. Flag serial bottlenecks: a manager that every worker waits on, waiting on a human, waiting on CI.
4. **Queueing.** Inventory age and work in progress per lane. Lead time grows with work in progress (Little's law), so long integration queues are a cost even when they cost no tokens.
5. **Context economics.** Spend and latency by context band, and quality by context band. This decides whether to cap context, compact earlier, or relaunch fresh sessions.
6. **Rank opportunities** by (avoidable cost × confidence) ÷ effort to fix. Each opportunity becomes an intervention hypothesis (§10).

## 9. Headline ratios and evidence grades

- **Cost per B2 change, cost per B3 outcome, and value per unit cost (B4).**
- **Escape rate.**
- **Claim-truth rate.**
- **Waste share.**
- **Inventory age.**

All of these are reported per lane, model, role, configuration and period.

| Grade | Meaning |
|---|---|
| A | Deterministic, verified end to end, with stated coverage |
| B | Semantic, with the classifier calibrated against human adjudication, and an interval attached |
| C | A proxy, a heuristic join, uncalibrated labels, or incomplete coverage |

Never compare numbers across grades. Never let a C-grade number drive a decision without a plan to upgrade it.

## 10. Causal claims: interventions

Logs show correlations; context size, for example, also tracks task difficulty. For each candidate improvement:

1. **Pre-register** the hypothesis, the primary metric, the minimum effect worth acting on, and quality guardrails.
2. **Randomise at the task level** within the same period. Never compare before with after alone.
3. **Size the sample** for the minimum effect before you start.
4. **Analyse by intention to treat.** Report effect sizes with intervals.
5. **Stop early** if a guardrail is breached.

Typical intervention families:
- context budgets and compaction policy;
- model and tier routing;
- manager/worker topology and fan-out;
- verification policy;
- batch size and integration cadence;
- tool affordances;
- briefing templates.

## 11. Instrumentation requirements

1. **Complete session capture** across all harnesses, with lineage (parent session, spawn reason).
2. **Join keys** embedded in artifacts: a session ID and lane ID, for example as commit trailers or PR metadata.
3. **Billing export** per model and per key, daily.
4. **Work-item registry**, with value labels.
5. **Review UI** for L2: queues of disagreements and calibration samples only, with the evidence packet shown beside the label form.
6. **A scheduled ASI job:** L0 extraction, then L1 classification, then the L2 queue, then the ledgers and ratios with their grades.
7. **Privacy.** Raw sessions stay in controlled storage. Only aggregates and labels leave it.

## 12. Reporting checklist

- [ ] Coverage: the share of sessions, lanes, work items and changes observed.
- [ ] Join method and its estimated error.
- [ ] Censoring windows respected; inventory reported separately.
- [ ] Cost from billing, or explicitly labelled as tokens.
- [ ] n and a confidence interval on every rate.
- [ ] Evidence grade on every number.
- [ ] Classifier calibration current for this corpus and configuration.
- [ ] Causal language only for randomised interventions.

## 13. Anti-patterns

- Labelling individual events by hand.
- Reporting tokens as cost.
- Treating unfinished work as failure, or counting it as delivered.
- Using LLM judges without calibration, or letting judges see the signal they are scored against.
- Capping evidence in a way that drops high-risk files.
- Counting planned follow-ups or design changes as defects.
- Comparing across evidence grades, or across periods with different coverage.
- Optimising a cost driver without a quality guardrail.
