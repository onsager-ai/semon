# Native session source readers

`SessionSourceReader` is a provider-neutral per-request callback for native
source ranges. It supplements the logical `SessionSourceRef` identities emitted
by the focused catalog. It provides no authorization grant or raw HTTP route.
The host selects the authorized machine and source generation; Semon retains
native record and transcript rendering rules.

```rust,ignore
fn read_range(
    &self,
    source: &SessionSourceRef,
    expected_generation: Option<&str>,
    offset: u64,
    max_bytes: usize,
) -> std::io::Result<SessionSourceRange>;
```

One callback request is limited to128KiB. A range returns its independently
bound generation, logical length, actual offset, immutable bytes and cached
observation flag. The range deliberately has no Serialize implementation, and
Debug prints byte length rather than source content. Native bytes must feed
parsed views under the applicable source/forensic permissions; ordinary trace
responses must not expose the original record bytes or their carrier labels.

A source reference is provenance, not bearer authority. The host reauthorizes
every callback, including byte-cache hits, against the request, current machine
registry, logical root/path, native identity and source generation. First reads
may resolve the verified generation; later chunks supply it explicitly and
require the same generation before and after I/O. The callback must reject
missing, ambiguous, stale or independently unverified source bindings. The
consumed-prefix SHA covers `[0,source.offset)`. A verified whole archived-object
SHA binds that prefix directly only when the consumed offset equals the object
length. An incomplete prefix needs its own independently retained proof;
reading a small range must not trigger a complete-prefix rehash or silently
claim a different object as the requested snapshot.

The reader belongs to one authorized request. Pass it by reference into the
parsed renderer; never retain it or its credentials in a shared ViewerCore,
workspace model, source cache or persistent projection. Async hosts run native
rendering outside their request runtime and bridge their existing bounded I/O
service. The reader captures the request's whole-operation deadline and
cancellation signal: a per-chunk timeout alone can multiply across a large
native line, and blocking rendering does not automatically stop when the client
disconnects. Keep diagnostics sanitized; provider URLs, object keys and
credential errors must not leak through parser failures.

The native-line oracle requests bounded chunks, pins generation and length,
rejects incorrect offsets, overlarge/empty responses and cross-generation reads,
and reads only complete consumed lines. It reuses the local transcript's JSON
object parser and line budget. Tests cover a three-chunk record, generation
replacement, trailing partial bytes and a reference at the consumed boundary.

This checkpoint establishes the callback contract, not archived Viewer
activation. The production per-request renderer registration, versioned
persistent transcript slot projection, indexed range loading, source/relationship
resolution and full native page/tool oracle comparisons remain pending. A full
selected FileIndex decode must not become the normal per-page fallback.
