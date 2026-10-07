# Native JSON string field ranges

`semon_sessions::json_string` separates the producer's complete native-record
validation from request-local decoding. Version 1 descriptors use offsets
relative to the original record, excluding quotes. Scalar checkpoints include
both field boundaries and are separated by at most 64 KiB + 11 raw bytes.
Escaped UTF-16 surrogate pairs form one boundary unit.

`index_json_string_spans` walks syntactically valid JSON once without allocating
string values. It returns RFC 6901 pointers and offsets. The caller must qualify
the native record and retain only explicit native field pointers. Arbitrary
object keys can contain user text and must never be blindly persisted. Duplicate
keys, excessive depth, field counts or pointer sizes return explicit unsupported
errors. Structured body fields have no string descriptor for the body itself;
embedded JSON wrappers and native formatting remain separate qualifications.

`decode_json_string_chunk(bytes, at_end, max_decoded)` accepts bytes beginning at
a verified scalar boundary. It decodes only that fragment and allocates at most
the requested output budget (1..=128 KiB). `consumed` counts raw bytes; the next
cursor adds it to the prior raw offset. Partial UTF-8, escapes and surrogate pairs
remain unconsumed until the next bounded read. `at_end` means the fragment reaches
the indexed field end, rather than simply reaching a provider response boundary.
A budget unable to hold the next scalar is an explicit error, preventing a
zero-progress retry loop.

The containing projection binds record offset, field span and cursor to the
source identity and independently verified source generation. These functions
provide neither source authorization nor generation verification. Callers use
a stored checkpoint or a prior verified chunk cursor; a supplied arbitrary raw
offset is insufficient. Provider requests must have their own byte limits.
When starting at a nearby checkpoint, discard only the bounded decoded distance
to a requested position. Never read a complete prefix to recover a cursor.

Tests compare all fields in committed Claude blocks and Codex legacy/early-item
fixtures with the shared native transcript parser. Escapes and literal UTF-8 are
split at every input-byte boundary, including surrogate pairs. A large result
fixture grows from approximately 2 MiB to 20 MiB while requesting a 4096-byte
chunk from a late checkpoint using at most 32 KiB of source input. Every
checkpoint segment independently decodes to the same complete-field oracle.
These are field-decoder bounds, not measured end-to-end Viewer budgets. SQL
publication, bounded provider access and native tool rendering must integrate
these descriptors before a complete large-result journey can claim those bounds.
