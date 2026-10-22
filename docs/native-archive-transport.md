# Native archive transport

Hub #304: native `.jsonl.zst` history participates in the ordinary `push` and
`push --watch` input set. Homes are opened read-only through the existing
descriptor-relative no-follow reader. The product needs no external decompressor,
substituted home or per-batch launcher. The receiver sees ordinary redacted
logical `.jsonl` bytes and retains the existing source identity, lineage, native
pagination/proof, custody generation, authorization and quota contracts.

## Reading and synchronization

`inputs` discovers supported archives beneath the same harness roots/depth/path
limits as the plain logs. The receiver's `is_input_path` allowlist is unchanged:
compressed paths are local transport sources, not a new wire/storage format.

The sender validates a complete Zstd stream, including concatenated and skippable
frames, before admitting its view to the normal complete-record/redaction sender.
Corruption, truncated frames, trailing garbage, source changes and resource-limit
failures leave that source unavailable and withhold the new full inventory.
Other sources continue. Valid Zstd containing an incomplete JSONL record may
transfer only its complete logical records; later appended frames can finish the
record. A stable corrupt-source witness is remembered for the running client so
watch does not repeatedly decompress the same bad history; changing/repairing
the source admits a fresh attempt. Restart conservatively verifies it again.

The logical path strips exactly the final `.zst`. Coexisting plain/archive files
must agree over every common byte before the longer source is selected. Equal
representations produce one mirror source and one inventory entry. A conflicting
prefix is reported rather than silently replacing one history with the other.
Active/archive moves retain existing native-ID selection and proof rules;
compressed input grants no extra inode-based relocation authority.

An archive's logical target freezes when its verified view first becomes ready
in a sweep. Re-preparation during later batches cannot advance that target;
growth is transferred in the next sweep, just as with plain native logs.

Preparation runs inside the existing foreground/history scheduler, with at most
1 MiB of encoded input, 1 MiB of decoded output and 256 decoder operations per
turn. Comparing coexisting encodings reads at most 1 MiB from each per turn.
Preparation turns that do work consume the watch pass budget; sources waiting
for a decoder/comparison slot do not. Active jobs therefore keep progressing
when newer archives are discovered. A pending successful catchup pass resumes
immediately, retaining foreground/history fairness, rediscovery, stop checks and
remote rate/pause backoff; the two-second poll applies after catchup or failure.
At most four decoders
and four unfinished comparisons hold descriptors; each decoder admits at most
an 8 MiB window. Cache usage is capped at 20 GiB of logical completed/unfinished
data. A cache-full error preserves original logs and receiver contents.

Views live in `<state>.archives/`, mode 0700, with mode-0600 files. They contain
unredacted local bytes, under the same private custody as the original homes.
The cache is intentionally a seekable plain transport view: bounded preparation
requires local space once, rather than repeated whole-archive decompression for
each send/read. This does not introduce another receiver storage compressor.
The existing seekable Zstd sealed-log storage remains the receiver's storage path;
sender source reads also use its logical `LogFile` reader.

Completed views are fsynced and atomically published only after descriptor and
named-source witnesses agree. A completion marker binds source and cached-view
identity, length, modification time and filesystem change time. Restart reuses a
valid view and the existing acknowledged logical cursor. Interrupted temporary
views have no completion proof and are rebuilt, in bounded turns, after restart.
Rewrites or changed cached bytes invalidate the view. Dropping/withdrawing an
unfinished source closes its descriptors and releases its cache reservation.
Completed views are retained within the cap; deleting the optional cache while
the pusher is stopped only requires bounded re-preparation on its next run.
Filesystem change-time witnesses are required (Unix); unsupported filesystems
fail explicitly rather than repeatedly regenerating unprovable cache entries.

No original log is modified, removed, sealed or renamed. Old receiver copies are
retained by the existing protocol. The sender's existing state lock owns cache
mutation and outlives outstanding upload/facts workers as before.

## Upload audit and measurements

Before and after this change, native `Client::post_wait` serializes JSON containing
base64 file bytes and calls reqwest `body`; it sets no `Content-Encoding` and has
no request compressor. Hub's append route consumes bounded bytes as JSON. Hub's
`CompressionLayer` applies to responses. Neither response gzip/Brotli nor sealed
Zstd segments establish compressed uploads.

`python3 scripts/benchmark-upload-compression.py --mib 16` compares identity,
gzip level 1 and Zstd level 3 with actual HTTP/1.1 loopback requests, the native
Append envelope, JSON/base64 decoding, append/fsync before acknowledgement, 100
live appends and 100 subsequent 4 KiB reads. Each mode runs in a fresh process.
The corpus comprises 34 checked-in, non-private compatibility JSONL fixtures,
224,081 source bytes, repeated to 16 MiB; SHA-256 of the fixture concatenation:
`3c52925bf994827cb8f5ee66a7721d11f135cfab092e5603464a09785d921d7d`.
CI runs the same experiment at 1 MiB and emits JSON; timing thresholds are not
used as flaky correctness gates.

Five local trials on 2026-10-22, Python 3.12.3/Linux, median per metric:

| Encoding | History wire bytes | History MiB/s | Process CPU s | Codec encode/decode s | Peak RSS KiB | Live p50/p95 ms | Read p50/p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| identity | 22,395,472 | 15.93 | 0.262 | 0.0003 / 0.0003 | 73,228 | 5.24 / 6.29 | 0.101 / 0.160 |
| gzip-1 | 5,736,701 | 14.74 | 0.511 | 0.158 / 0.079 | 73,508 | 9.99 / 18.33 | 0.100 / 0.265 |
| Zstd-3 | 4,034,838 | 12.62 | 0.505 | 0.081 / 0.111 | 84,940 | 11.66 / 20.60 | 0.078 / 0.161 |

Wire counts include request headers and encoded bodies as actually sent through
the socket API, before TCP/IP/TLS framing; the experiment uses loopback HTTP.
CPU is the reference sender/server process combined, with per-thread codec CPU
also measured. RSS includes Python and its bounded reference decode buffers, not
native product memory. This is an experimental receiver, not Hub's PostgreSQL,
authorization, quota, Index, sealed storage or production network path. The host
also ran Rust checks and an original-home watch; latency/throughput varied with
contention. Corpus repetition can favor compression. Subsequent reads load the
same plain logical bytes in all modes and do not establish a storage speedup.

Gzip and Zstd reduced measured upload wire bytes by 74.4% and 82.0% here. They
increased CPU and did not establish a throughput or live-latency improvement on
this host. Keep **identity as the supported/default upload encoding** in this
change. No upload negotiation or retry with another encoding is introduced:
existing receivers remain compatible, and an uncertain external write is never
replayed merely to change codecs. A future compressed-upload option requires
explicit receiver capability admission, independent encoded/decoded caps before
body inflation, unchanged logical quota charging and measured deployed-network
benefit. A response's `Accept-Encoding`/`Content-Encoding` is not that capability.

## Validation

Native tests cover corrupt/truncated/concatenated/skippable streams, window/output
caps, symlink refusal, changed source/view witnesses, interrupted preparation,
restart reuse, equal/prefix/conflicting representations, incomplete records and
discovery/live appends in one continuing native watcher. Compatibility fixtures
compare source bytes, native MCP Query pagination, Web generation-bound pages,
lineage and usage metadata after redaction and restart. The paired Hub regression
uses a real native watcher, authenticated HTTP and restricted-role PostgreSQL;
it covers Web/MCP pagination, tenant isolation, new archives, continued appends,
restart without Raw replacement and existing sealed storage.

Original-home and exact-pair validation receipts are recorded separately from
this synthetic CI corpus. No HP Code 1 transcript-read repair or production
deployment qualification is implied by these tests.
