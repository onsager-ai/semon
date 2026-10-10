# Historical Trace Store export

This migration command preserves the experimental Canonical Trace Store before
its Store, capture and replication code is retired. It does not export the core Session
Event Index, restore a harness workspace, or provide runtime recovery.

```sh
semon-forensic-export --store /private/history/traces.sqlite3 \
  --out /private/history/trace-export-v1
```

The standalone tool owns this offline export contract. It builds independently
of `semon-store`, capture adapters, Sessions, Viewer, Relay and Runtime:

```sh
cargo build --locked --release -p semon-forensic-export
```

The release-binary workflow supplies a separate
`semon-forensic-export-linux-x86_64` artifact. Neither build nor download starts
capture or reads historical files. The main CLI's historical `log`, `forensic`,
`forget` and export alias are retired. The qualified pinned legacy CLI preserves
those contracts for existing data; see the procedure below. Normal session
commands have no Canonical Trace Store dependency.

Pause capture writers and other access to that store during the snapshot. The
command never stops services, uninstalls timers, changes original permissions,
migrates the original schema, updates cursors, or deletes original files. Keep
the original DB and any WAL, SHM and rollback journal together. SQLite's
[WAL documentation](https://www.sqlite.org/wal.html#the_wal_file) explains why
committed evidence may still live in the WAL.

The destination must not exist; its parent must already exist. On Unix, the
command creates the directory with mode `0700` and files with mode `0600`.
The forensic warning is printed before output. No raw records are
printed to stdout. Selected `--session`, `--trace`, `--day` and `--out` modes
remain available in the pinned CLI and cannot be combined with its `--export-store`.
The standalone tool exports the complete store only and requires both explicit
paths; it has no capture, synchronization, network, pruning or deletion command.

Original files are opened only through read-only file handles. Private input
copies must match the originals' full SHA-256 hashes and file generation stamps
through snapshot verification; changed, replaced, added or removed files reject
publication. SQLite then operates only on the private copy. Its
[backup API](https://www.sqlite.org/backup.html) creates a self-contained database
with committed WAL state, followed by an integrity check. Temporary input copies
are removed after success; the originals remain untouched. This is an offline
export contract; file checks do not authorize stopping a writer or replace the
operator's quiescence requirement.

Export supports the historical Store schema versions `0..=7` and refuses newer
versions or a database without `canonical_traces`. It copies every table and
schema object without reinterpreting rows: canonical JSON, occurrence identities
and relationships, raw/unprojected/unlinked bytes, trace links, capture ownership
and source-revision custody. A missing forensic table stays missing. It never
recreates deliberately removed data.

A successful directory contains:

- `traces.sqlite3`: the complete logical SQLite snapshot, in rollback-journal mode.
- `manifest.json`: format `semon.forensic-store-export`, version `1`, the source
  schema version, `includes_forensic: true`, database filename/length/SHA-256,
  each table's name and row count, and input-file hashes. `includes_forensic`
  identifies the explicit forensic export mode; a previously removed raw table
  is not fabricated.

The manifest is the completion marker and is published only after verification
and file synchronization. Errors can leave a private incomplete directory
without `manifest.json`. Retain the original, investigate the error, and retry to
a new destination; no cleanup of existing user data is performed.

Verify a completed artifact before inspection or transfer:

```sh
python3 - /private/history/trace-export-v1 <<'PY'
import hashlib, json, pathlib, sqlite3, sys
root = pathlib.Path(sys.argv[1])
manifest = json.loads((root / "manifest.json").read_text())
assert manifest["format"] == "semon.forensic-store-export"
assert manifest["version"] == 1 and manifest["includes_forensic"] is True
assert 0 <= manifest["source_schema_version"] <= 7
assert manifest["database"]["file"] == "traces.sqlite3"
database = root / "traces.sqlite3"
for suffix in ["-wal", "-shm", "-journal"]:
    assert not database.with_name(database.name + suffix).exists()
with database.open("rb") as content:
    digest = hashlib.file_digest(content, "sha256").hexdigest()
assert digest == manifest["database"]["sha256"]
assert database.stat().st_size == manifest["database"]["bytes"]
with sqlite3.connect(database.resolve().as_uri() + "?mode=ro", uri=True) as db:
    assert db.execute("PRAGMA integrity_check").fetchall() == [("ok",)]
    assert db.execute("PRAGMA user_version").fetchone()[0] == manifest["source_schema_version"]
    names = [row[0] for row in db.execute("SELECT name FROM sqlite_schema WHERE type='table' ORDER BY name")]
    assert names == [table["name"] for table in manifest["tables"]]
    for table in manifest["tables"]:
        name = table["name"].replace('"', '""')
        assert db.execute(f'SELECT count(*) FROM "{name}"').fetchone()[0] == table["rows"]
print("Verified complete forensic Store export v1")
PY
```

## Legacy inspection and deliberate deletion

Preserve the [qualified pinned CLI/source](trace-capture-retirement.md#pinned-legacy-tools)
and its checksum before replacing an installed binary that still serves
historical commands. Inventory jobs that invoke those commands: the current
binary refuses them before opening a database, and does not redirect them to
native session queries or mutate existing files. No installed binary, service,
database, cursor, key or Relay deletion queue is removed by source retirement.

Use SQLite read-only access on the verified exported file to inspect historical
tables. Legacy `log`, selected `forensic` and `forget` use a schema-migrating open
that can recreate tables or tighten permissions. Give those commands a separate
owner-private working copy; preserve the verified artifact and manifest intact:

```sh
mkdir -m 700 /private/history/inspection
cp /private/history/trace-export-v1/traces.sqlite3 /private/history/inspection/traces.sqlite3
chmod 600 /private/history/inspection/traces.sqlite3
/private/tools/semon-legacy log --store /private/history/inspection/traces.sqlite3
/private/tools/semon-legacy forensic --store /private/history/inspection/traces.sqlite3 \
  --session SESSION --out /private/history/inspection/excerpt.txt
```

The example assumes `/private/tools/semon-legacy` is the exact preserved binary,
not the current `semon`. Ordinary `log` reads only canonical traces/occurrences;
forensic access warns before output and creates excerpts `0600`. Session/day
selection includes the raw row's own unprojected lines. Trace selection returns
each complete linked source line once and cannot select unprojected rows.
Neither selected access nor the retired `ship` sender is a complete forensic
backup; use the all-table exporter and verifier above.

Only an explicit operator decision may delete historical records. The pinned
`forget --forensic` still requires exactly one selector and interactive
confirmation or `--yes`. Session/time selectors operate on each raw row's own
provenance; trace selection deletes every complete linked raw line across
sessions, including its other projected blocks. Canonical traces/occurrences
survive. Secure-delete plus vacuum removes freed SQLite payloads; deletion from
a working copy does not erase originals, exported artifacts, native logs or
other previously restored copies. No deletion or pruning happens automatically.
The pinned [forensic policy](design/forensic-retention-and-exposure.md) records
the full historical selection and exposure contract.

For existing queued/coordinated Relay deletion, preserve the original sender
state, endpoint origin, enrolled signing/age identity and trust configuration.
The pinned CLI requires explicit `--relay-endpoint`, `--relay-state` and
`--relay-config` (and pinned HTTPS `--relay-ca` when configured), queues the
request before local deletion and keeps offline requests pending. Semantic trace
IDs cannot select carrier frames. The dedicated `semon-relay forget` remains
available at this stage. Local-only deletion leaves server copies; neither
source retirement nor export acknowledges or drops a pending request. Follow
the [Relay custody contract](encrypted-relay-retirement.md) before replacing
those tools or making a separate explicit remote deletion decision.
