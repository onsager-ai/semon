# Blind annotation quick reference

This restates the frozen taxonomy. It contains no rules, predictions, or results.
Classify the marked current event, using only the evidence provided by the
controlled interface. A quoted past failure does not itself establish a current
failure. Redacted evidence may support `unknown`.

Submit exactly five fields, with no ID or `gold` wrapper:

| Field | Allowed values |
| --- | --- |
| `activity` | `task_request`, `progress`, `tool_action`, `wait`, `failure`, `recovery`, `completion`, `unknown` |
| `failure_category` | `none`, `tool`, `permission`, `environment`, `model`, `transport`, `malformed`, `unknown` |
| `retry_loop` | `none`, `retry`, `loop` |
| `anomaly` | JSON boolean `true` or `false` |
| `phase` | `start`, `active`, `blocked`, `recovery`, `ending`, `unknown` |

Activity distinguishes a user instruction, ordinary agent progress, tool action
or result, waiting, current failure, resumed useful work after blockage, and
reported completion. Completion means the assistant reports the current
requested work or turn complete; a stop marker alone is insufficient.

Failure categories distinguish no evident failure, failed tool/command,
authorization denial, environmental blockage, model/output failure, transport
failure, malformed native event, and an evident failure of unclear category.

Retry requires an action retried after failure or an explicit retry cue. A loop
means the same action or failure repeats at least three times. Repeated routine
commands alone need not establish a retry; redacted identities can be ambiguous.

An anomaly is malformed or unexpected behavior or an anomalous repeated pattern
requiring investigation. An ordinary tool failure alone is insufficient.

Phase distinguishes session start, ordinary progress, waiting/blockage,
recovery, and session termination. Ending requires native terminal evidence;
last captured row, turn stop, and reported task completion alone are insufficient.

Use `real_label.py next --annotator A` (or B), then submit one JSON object on
stdin to `real_label.py submit --annotator A`. Each submission saves the label
before revealing the next prefix. Never inspect raw sources, rules, predictions,
future sessions, or another labeller's answers. Offline access opens only after
both causal passes finish; review each full session before submitting changes.

An adjudicator uses `real_adjudicate.py next` / `submit` for causal disputes,
then `offline-next` / `offline-submit --session-id ID --reviewed`. Every offline
dispute needs an explicit full label, including deliberate unknown decisions.
All corpus, annotation, audit, and example files remain local and private.
