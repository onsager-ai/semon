# Worker134 current witness WIP

Base b0380be7c5edba4107cf7f29b22a4ec89d5db9f0 was clean and matched
origin/task/479-pg-foundation-20261025 before edits. Worker132 failed on
provider capacity after real WIP; it is not accepted or replayed here.

The current witness uses domain-separated fixed-width SHA-256 key addresses
and a length-framed SHA-256 stream, tagged tree4. SQLite stores generated
addresses and indexes them; PG uses matching generated addresses behind its
namespace-digest leading key. A source's namespace is authenticated by the
captured header and source owner; it does not permute the transferable witness
order. Full original keys and values remain in the heap and witness. Public
maps keep logical ordering. This is neither an XOR nor an additive checksum.

Fresh-only schema versions: SQLite 34, PG 9. No upgrade/reset or old reader.
This is intermediate WIP: current producer/readers/fixtures, whole-dimension
defer, closed-name authentication and the complete resource/fault matrix still
need qualification. No local checks ran: thermal telemetry is unavailable.
Root collects exact CI. Full backend qualification remains zero.
