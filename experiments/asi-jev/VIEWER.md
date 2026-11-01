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

## Account-private ChatGPT copy

The cloud bundler preserves the existing review gate and embeds the five viewer
files in one HTML file. It does not rerun or change the experiment:

```sh
python3 experiments/asi-jev/cloud_viewer.py
```

The result is `private/cloud-viewer/index.html`. For an aggregate-only bundle,
use `--aggregate-only`; this reads only public aggregates and contains no session
text, labels or individual predictions. Both modes write to the same dedicated
private output file, so choose the mode explicitly before delivery.

Cloud publication requires separate authorization for the exact content and a
verified owner-only Site audience. The full bundle contains sanitized individual
session text. Never put it in Git, a public/link-shareable Site, or CI artifacts.
Keep deployment output ignored, and commit only generic source code to any Site
repository. Recheck the exact payload and audience before uploading. The current
user authorization allows the existing reviewed full projection, not raw logs or
new data. An existing account-private publication does not authorize widening its
audience.

The bundle makes no external requests. Review drafts live only in browser storage;
download JSONL to keep the answers. There is no server-side answer sync. Embedded
results are read only after the reveal step; this remains a UI blinding aid,
not protection against inspecting the HTML source. Synthetic CI browser checks
exercise both local and bundled versions, including script injection safety,
downloads, all seven views, and delayed reading of embedded results.

## Phone-first review

The phone layout prioritizes one event card with the frozen reference and rare
class flags. Context is collapsed until expanded, wraps at the screen width, and
uses readable monospace. Decision buttons stay at the bottom; previous/next and
progress preserve the current event. Browser drafts remain keyed to the same
review packet, including when moving from the earlier desktop layout.

Download the review file and confirm that you saved it before revealing results.
On browsers with native file sharing, the Share option can send the same JSONL
file to Files or another destination. A cancelled share is not a completed export.
The site cannot verify the OS save location. Browser-engine checks include WebKit
with an iPhone viewport; these do not replace testing the native iOS share sheet
on an actual device.

The current mobile requirements intentionally replace the previous dense
desktop layout on phones: stacked data cards, vertical condition comparisons,
tap details on confusion cells, a vertical event timeline, and filter sheets.
The existing palette, experiment data and review/reveal semantics are preserved.
Synthetic screenshots cover every view at 375×812, 414×896 and desktop. No
personal session text is needed in visual verification or CI artifacts.

The generator bounds each input read below 10 MB and checks actual CPU
temperature before and after work. It stops at 85°C or unavailable telemetry.
Synthetic generator and browser checks run in CI, with no private inputs.
