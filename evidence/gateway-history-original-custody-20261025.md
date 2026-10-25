Gateway correction after Worker64, test only; no acceptance is claimed.

Exact CI on 89de03a7406528d532613ba5eda9e2a6fe862d7f passed 32 SQLite,
3 journal and 72 of 73 configured tests. The remaining new late-child test
expected composite History, but the actual retained parent is its original
observation, including Query facts. The 90-group was not reached.

The source rule is explicit: storage/sqlite/history.rs:69-79 returns the old
admitted Current row whenever its serialized metadata differs. The publisher
calls preserve_replacement_rows before publish_history_rows; a completed
original makes the latter skip composition. Worker64's composite-only oracle
missed that preflight. Its original handoff remains unchanged as history.

The fixture already asserts empty prior History. The corrected oracle derives
the exact original-custody set by comparing each prior Current observation to
the independent final producer model. Every changed observation must retain
its complete prior metadata. Exact map equality, one retained parent, full
Query/source/prefix metadata equality, retained recipe header and slot-prefix
existence replace the incorrect composite/disappeared-source assumptions.
Current=4, children=3, Current proofs=4, foreign state, reset rollback, stale
model Conflict and indexed late-child assertions remain unchanged.

No production code, schema, adapter, proof lifetime, PG or S2 work changed.
No local build/test/formatter ran: thermal telemetry is absent. Exact-head CI,
its formatting artifact and fresh Manager review remain required. Full #477,
PG, migration, Hub313 and the #380 gate remain pending.
