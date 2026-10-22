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

## Reference receiver prototype

Before and after this change, native `Client::post_wait` serializes JSON containing
base64 file bytes and calls reqwest `body`; it sets no `Content-Encoding` and has
no request compressor. Hub's append route consumes bounded bytes as JSON. Hub's
`CompressionLayer` applies to responses. Neither response gzip/Brotli nor sealed
Zstd segments establish compressed uploads.

`scripts/benchmark-upload-compression.py` is a **reference receiver prototype**.
It compares identity, gzip level 1 and Zstd level 3 with loopback HTTP/1.1,
the native Append envelope, JSON/base64 decoding, append/fsync before
acknowledgement, 100 synthetic appends and 100 subsequent 4 KiB reads. Each
mode runs in a fresh process. It does not exercise the pusher process or Hub's
PostgreSQL, authorization, quota, Index or sealed storage paths.

The retained prototype snapshot used 34 checked-in compatibility JSONL files,
224,081 source bytes, repeated to 16 MiB. Its concatenation hash and local
numbers below describe only that older prototype run; they are not the current
corpus fingerprint or product upload evidence. CI continues to run this small
reference experiment as a labeled prototype, with no timing threshold.

Five local trials on 2026-10-22, Python 3.12.3/Linux, median per metric:

| Encoding | History wire bytes | History MiB/s | Process CPU s | Codec encode/decode s | Peak RSS KiB | Live p50/p95 ms | Read p50/p95 ms |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| identity | 22,395,472 | 15.93 | 0.262 | 0.0003 / 0.0003 | 73,228 | 5.24 / 6.29 | 0.101 / 0.160 |
| gzip-1 | 5,736,701 | 14.74 | 0.511 | 0.158 / 0.079 | 73,508 | 9.99 / 18.33 | 0.100 / 0.265 |
| Zstd-3 | 4,034,838 | 12.62 | 0.505 | 0.081 / 0.111 | 84,940 | 11.66 / 20.60 | 0.078 / 0.161 |

These prototype wire counts include request headers and encoded bodies before
TCP/IP/TLS framing. Its process CPU combines the reference sender and receiver;
RSS is Python process memory. The results are sensitive to corpus repetition
and host contention, and do not establish Hub or production throughput.

The prototype reported lower encoded bytes and higher CPU on its local fixture
snapshot. Those figures do not change the product protocol. **Identity remains
the only supported upload encoding**; gzip and Zstd appear only as unsupported
benchmark prototypes. No upload negotiation or retry with another encoding is
introduced. A response's `Accept-Encoding`/`Content-Encoding` does not advertise
upload support.

## Product upload benchmark

The paired Hub benchmark starts the built native `semon push --watch` child
against real Hub HTTP and restricted-role PostgreSQL. Its current corpus,
method, CI report and limits are documented in
[Hub native archive ingestion](https://github.com/onsager-ai/semon-hub/blob/fix/native-zstd-archives/docs/native-archive-ingestion.md).
That report is separate from the reference receiver prototype above. Identity
remains the only supported upload encoding; the candidate gzip/Zstd paths are
test-only shims and introduce no product negotiation or fallback.

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
