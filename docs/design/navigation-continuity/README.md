# Shared destination continuity

The core destination registry is `ui/src/lib/navigation.json`. Rust fallback and
the compatibility, source-discovery, focused-catalog and native page owners all
project it. Generic host policy supplies leading/trailing entries and URL
overrides; the public application does not depend on a particular host product.
Native page chrome adopts server content without constructing the complete reader.

Source selection is Sessions context. Its real URL/history step retains per-source
transcripts, loaded ranges, drafts, filters, focus and scroll. Focused Recent is
labelled “Recent on this machine”. Home and Analytics explicitly enter the
supported compatibility reader. Shared account/drawer ownership and modified-click
fallback remain with `createShellChrome`; narrow navigation subscriptions belong
to each document lifetime. No store, router or poller is added.

The synthetic [desktop list](list-1280-light.png) and
[phone transcript](transcript-390-dark.png) retain the existing fonts, tokens,
geometry and controls. These captures document the new destination projection;
they do not replace approved pixel references or relax comparison thresholds.

Local verification: 92 UI unit tests; 12 focused browser cases across phone and
desktop in both themes; 13 shared browser lifecycle cases; nine application
lifecycle cases (including native content teardown and zero model requests);
the unchanged embedding sidebar parity check; the actual compiled
bounded-producer browser suite; types, architecture, design policy, generated
freshness, bundle sizes, Rust formatting and all-target Clippy. Full locked Rust
testing reached 528 passing session tests and one failing archive-snapshot test:
`viewer::tests::a_refused_archive_root_replaced_by_a_directory_invalidates_the_snapshot`.
The exact failure also reproduces at baseline
`282007f96c60061a9e8539b5c7570909ebeb61fe` on this filesystem. It remains a failing
gate. The full compatibility pixel aggregate was not run locally.

Ownership review: library code imports no application state; host policy is
validated at its boundary; chrome owns only its existing descendants and keeps
native content slots intact; subscribers, requests and listeners are released
on teardown. Focused browser fixtures throw on complete-model access and retain
native transcript node identity through source changes and Back/Forward.
