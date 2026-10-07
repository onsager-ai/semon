# Actual Viewer journeys with a constant page and conversation

The embedded catalog Viewer keeps persisted restart, filtering, selection,
transcript ranges and large native fields responsive while unrelated history
increases. First publication and background producer work still grow with the
complete selected machine. This workload does not qualify a bounded producer or
an end-to-end latency SLA.

The measured public source is
`733524fad75290d05aae887faa0c00263b11c6f7`. The test-clock CLI SHA256 is
`ef6014187907bec42020e81d51cef33be90a5be38b7ed893fd8969ce455e11ec`; every
browser run independently verified its embedded `/viewer.js` SHA256
`0e8f41c04320954b89c972f135144b4ac199591b62604d650f44581f540765ed`.
No generated bundle was intercepted or replaced. Chromium/Playwright runs
against real native fixture files, the CLI HTTP server and public query routes.
There is no provider or guest runtime artifact in this standalone workload.

The existing native fixture contains55 sessions across harnesses; the selected
Claude conversation has473 records after adding a110029-byte scalar.64 fixed
older Claude sessions fill the first60-row page, and0/256/2048 additional older
sessions each contain8 records and about28KiB. Requested source `local`, first
page60, selected `backlog`, latest transcript60, earlier transcript60 and scalar
remain fixed. Input Claude files total109/365/2157 and2.54/9.82/60.80MB. All
six cases have60 list rows,60 initial entries and120 after loading earlier.
Fixture directories have different path lengths, explaining small provenance
response-size differences.

Each workload starts once with no projection (first import), then restarts the
process over the published projection. These are process-cold starts with warm
OS file caches; `read_bytes=0` in these samples. The same browser interaction
sequence runs against each process: first useful list DOM, commit a harness
filter, select the conversation, load an earlier range, fetch both scalar chunks,
return to the list, and return to the retained conversation. Assertions verify
the exact original scalar, retained original DOM node, no page errors and no
legacy model/tool/image/tx requests.

| Added sessions | State | First useful list ms | Filter ms | Select ms | Earlier ms | Both field chunks ms | Return ms |
| ---: | --- | ---: | ---: | ---: | ---: | ---: | ---: |
|0|First import|1416|83|143|85|183|81|
|0|Persisted restart|182|89|126|108|258|56|
|256|First import|3430|102|137|98|203|63|
|256|Persisted restart|154|60|153|69|257|83|
|2048|First import|15483|83|137|96|198|56|
|2048|Persisted restart|248|78|136|86|175|74|

These are six individual samples, not percentiles. Earlier diagnostic replays
also exposed the same first-import growth; they are excluded from this table.
Wall time includes actual initial503 recovery before publication. The comparison
is before versus after persistent publication using identical code, not a
historical before/after software speedup claim. No earlier matched legacy
binary+UI complete-journey baseline was established by this experiment.

| Added sessions | State | Sampled peak RSS MiB | Server CPU seconds | Server rchar MB | Server writes MB |
| ---: | --- | ---: | ---: | ---: | ---: |
|0|First import|45.7|1.00|29.0|7.23|
|0|Persisted restart|42.8|0.75|22.5|0.01|
|256|First import|73.1|2.56|84.1|17.24|
|256|Persisted restart|71.3|1.13|58.4|0.00|
|2048|First import|251.9|12.24|508.5|113.61|
|2048|Persisted restart|56.6|1.18|68.7|0.00|

Linux `/proc` samples include asynchronous producer work, not just endpoint
latency; CPU uses100 clock ticks/second. `rchar` counts reads served through the
page cache, including SQLite work, rather than physical disk traffic. RSS is
sampled every50ms and is not a retained-live-heap measurement. Work continues
through evidence collection. For first-import cases, canceled initial503 response
bodies remained unavailable to Playwright and are explicitly recorded as
`bytes:null`, `body_unavailable`. The bounded6s optional body capture can extend
server observation after the last user action; the per-action DOM timings above
do not include that evidence wait. There is no byte/RSS budget claim.

Successful first-list responses are64.8–64.9KB, selected ranges55.7–56.2KB,
and scalar chunk responses67.8KB/46.8KB. The selected60-record DOM has1120
nodes at every workload; selected JS heap is approximately4–5MiB, cumulative
script time14–22ms and layout time50–64ms. After120 entries and field expansion,
visible DOM has1654 nodes. Uncollected JS heap varies with garbage collection,
so it is not a memory ceiling. Full CDP/task/heap/node and response evidence is
in [the raw measurement directory](measurements/catalog-journey-733524f/).

The remaining constraint is concrete: first import waits for a complete
selected-machine observation and persistent restart still permits global
selected-machine background reads/model work. Bound incremental producer and
summary publication before qualifying history-independent total work or the
umbrella's proposed performance budgets. Separate runs must also qualify live
following, temporary provider/transport failures, multiple sources/viewers and
hosted archived-byte restoration; this workload covers retained catalog reads
with local original bytes present, not private archive/provider behavior.

Reproduce on Linux with Node and the repository Playwright installation:

```sh
python3 scripts/catalog-journey.py \
  --binary /absolute/path/to/semon-with-test-clock \
  --source-revision FULL_SOURCE_SHA \
  --counts 0,256,2048 \
  --output /new/empty/output/directory
```

The runner retains synthetic data/evidence, rejects reused fixture directories,
keeps ephemeral authorization manifests mode0600, removes them when each server
stops, and never writes tokens into the evidence. It records the binary hash and
server counters. The browser driver bounds response evidence capture, preserves
required exact field assertions, and reports unavailable optional body sizes
instead of manufacturing them.
