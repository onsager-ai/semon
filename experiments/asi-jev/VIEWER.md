# Local experiment viewer

This viewer reads saved experiment outputs. It does not run inference, change
the experiment, or modify its projection, reference, criteria or predictions.
All generated pages and data stay under gitignored `private/`. No external
scripts, fonts, analytics or network services are needed.

From the repository root, build the private viewer and serve only its dedicated
output directory:

```sh
python3 experiments/asi-jev/local_viewer.py
python3 -m http.server 8765 --bind 127.0.0.1 \
  --directory experiments/asi-jev/private/viewer
```

Open <http://127.0.0.1:8765/> on this machine. Stop the server with Ctrl-C.
Do not serve the checkout or the whole `private/` directory. Do not bind to
`0.0.0.0`, forward the port, deploy the output, or upload it as a CI artifact.
The generated private site includes sanitized session text and individual
annotations and predictions. It remains private even after the results reveal.

The first page shows the existing 50-event spot-check with its sanitized causal
context and frozen causal reference labels. Mark each event agree, disagree or
unsure, optionally add a note, and export the answers to a local JSONL file.
Browser storage holds a draft; it is not a replacement for saving that file.
Import a saved file to resume. Exported decisions are a review of the reference,
not corrected gold labels or automatic input to the frozen scoring pipeline.
The original reference is never rewritten.

Classifier results are fetched only after the explicit completion/reveal step.
This is a UI blinding aid, not an access-control boundary against someone
deliberately opening the result files. Once revealed, the user has seen model
performance and the review must no longer be treated as model-blind.

Results include criterion intervals, quality/latency and honest cost labels,
per-class precision/recall, confusion heatmaps, session phase timelines, a
disagreement explorer, the first feasibility run versus the real cohort, and
plain-language limitations. Repeated trials are shown separately in individual
views; aggregate confusion counts give each unique event equal weight by
averaging its repetitions. Missing model answers remain abstentions. Partial
builds are labelled incomplete; missing conditions are never interpreted as
passing or zero-cost.

For an aggregate-only site with no individual session data:

```sh
python3 experiments/asi-jev/local_viewer.py --aggregate-only \
  --output experiments/asi-jev/private/viewer-aggregate
python3 -m http.server 8766 --bind 127.0.0.1 \
  --directory experiments/asi-jev/private/viewer-aggregate
```

This mode reads only the public aggregate inputs. It has no spot-check,
session timelines, individual disagreements or event-derived confusion
matrices. It still requires an explicit reveal step. The mode is useful for
checking the generator without private data; it is not authorization to publish.

The generator bounds each input read below 10 MB and checks actual CPU
temperature before and after work. It stops at 85°C or unavailable telemetry.
Synthetic generator and browser checks run in CI, with no private inputs.
