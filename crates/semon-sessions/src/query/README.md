# Query contract reference

Embedding callers replace `Query::from_core(core)` with
`Query::from_native_core(core)`. The host still owns authorization, admission and
source custody; the native constructor preserves its configured output window.
CLI/stdio callers keep `Query::new` or `Query::with_machines`. Existing hosted
production pins keep their original API until the paired consumer adopts the
new Semon revision. No constructor alias or full-model fallback remains.

`contract-reference.json` contains 134 synthetic tool responses from the retired
production `Query::from_core` engine at Semon
`7ce1ac4d4f34af9143aa292b631a07fd797890b1`. It preserves the independent oracle
after the compatibility constructor and its model-reading implementation are
removed. It is test data; production does not load it.

The existing fixture families supply the requests and native evidence:

- 56 output-window cases: all five tools, 24-hour/six-hour windows, active sessions,
  retained links, one/two machines, paging, search budgets and exact errors.
- 78 scan cases: all five tools, one/two machines, native source rewrite and process
  disappearance, paging/turn anchors, search stopping and unknown-session errors.

The capture ran the parent's two comparison tests unchanged apart from recording
each legacy result before its existing assertion. Production `query.rs`,
`model.rs`, `viewer.rs` and `union.rs` matched the pinned commit byte for byte;
the instrumented tests passed. The clock is `1790294400000` (epoch milliseconds).
Responses contain synthetic identities/content and no checkout paths or secrets.
No compatibility engine or reference generator is retained in the product.

Current tests compare complete result values and error bodies against these
records, while checking zero compatibility builds and shared native cohorts.
The surrounding semantic fixtures still test identity, relationships, facts,
current/retained windows, closed-source retirement and source-backed
reads. Do not regenerate the reference from the current implementation merely
to make a changed result pass; an intentional public contract change needs its
own reviewed expectations.
