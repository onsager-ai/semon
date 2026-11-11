# Governance decision library

`semon-asi::governance` is the deterministic local decision layer for the #592
policy slice. Policies use strict format 1 JSON: unknown fields, unsupported
versions, unknown exported fields, and malformed typed scopes fail closed.
`PolicyBundle::content_version` hashes canonical baseline and local policy
content; callers cannot supply an authorization version.

`Scope` is a structured `(destination, namespace, machine)` tuple, never a
delimiter-joined string. `evaluate` accepts the trusted authenticated principal
and complete admitted source cohort at that exact scope, applies the stricter
baseline/local policy and source classification floor, and returns an opaque
`ExportAuthority`. Its fields are private: callers cannot manufacture a grant,
drop a contributor, change the principal, or replay a receiver grant to Hub.
`project_derived_row` is the sole row-export route and accepts only that authority.
It validates row existence, every contributor's field grant, schema types, source
cohort binding, and destination-specific values before returning a `GovernedRow`.
`serialize_governed_row` emits only its authorized typed fields. Every governed
row also retains a private, immutable `ExportWitness` that binds the authenticated
principal, exact scope, complete source IDs, policy versions, and scan/detector/
redactor evidence evaluated for that row. Trusted local publication or audit code
can inspect it through `local_witness`; it has no automatic serialization and its
debug output is redacted. `coverage_witness` and `serialize_coverage_witness` are
the separate route for provenance disclosure, and refuse unless every contributor
allowed coverage.

Receiver exports accept typed plain paths under the contributor intersection;
credential-bearing path values are refused. Hub accepts only `pt1_` path tokens,
and `body_read_allowed` is always false for Hub. Typed key/name fields reject
recognizable credentials and absolute-path laundering. Text keys follow their
declared contracts: native UIDs use ASI store percent-encoded components, repo
keys accept established remote/local identities such as `github.com/org/repo`,
and integer keys such as `date_key` remain integers. The library does not apply a
generic identifier rewrite or infer that arbitrary short prose is safe.

`fact_label` is deliberately refused by this slice. The analytics schema describes
its intended categorical fields, but it does not yet provide a supported,
versioned field/value taxonomy registry. Until the L1 contract can evaluate an
approved categorical set, `field`, `value`, and `labeller` cannot be used to
export a free-text assertion. No raw `StepFacts`, native facts, goals,
explanations, prompts, evidence excerpts, or other bodies are serializable.

When coverage is disallowed, callers must withhold the coverage row rather than
export counts, source handles, policy or scan versions, vector lengths, or
denominators. `coverage_witness` is the current explicit coverage-authorized
envelope; it omits the authenticated principal. Missing optional fields remain
withheld, while explicit `null`, unknown values, numeric zero, and empty strings
remain distinct when their typed field is authorized.

## Pinned receiver projection (#593 slice 2)

`semon-asi::egress` connects the decision library to the local Analytics Store.
`Snapshot::pin(root, namespace_file, namespace)` takes the existing shared store
lock, admits real directories and regular contained snapshot files using the
custody-export rules, reads one manifest/live map and its generation-bound
provenance, and decodes fresh typed rows before releasing the lock. Unknown tables,
columns, Arrow metadata and Parquet key/value metadata (except the writer's
`ARROW:schema`) refuse. Size, row-count, generation-range and live-key consistency
are checked. Later publications cannot alter the in-memory snapshot. Errors are
fixed enum messages with no underlying path, denied value or parser message.

The caller explicitly loads an operator-owned local JSON registry with
`Registry::load(path)`. Format 1 has `format`, `namespace`, and `sources`; `sources`
maps retained source `native_id` values to `{source, baseline, local}`. `source`
is the existing governance Source document (classification floor, owner, scope
and scan evidence); baseline/local are existing strict Policy documents. Its `id`
is replaced by the opaque local source handle. Policy versions come from
`PolicyBundle::content_version`, never configuration labels. There is no default
location, ambient source discovery, Catalog lookup, receiver-supplied policy, or
fallback grant. The trusted local caller supplies the authenticated Principal and
exact receiver Scope. Missing configuration refuses the operation; missing,
empty or duplicate retained contributor identities refuse that session.

`Handles::open(state_dir, namespace)` explicitly creates/opens
`state_dir/analytics-egress-handles.sqlite`, beside `analytics-namespace` and
outside `state_dir/analytics-store`. New files are created atomically with mode
0600 on Unix. SQLite random 32-byte identifiers map source and evidence/native
keys independently, bound only to the namespace. Moving/copying the same namespace
preserves origin identity; filesystem paths are not identity authority. Keep the
mapping for the namespace lifetime. Reset with the namespace kept preserves it;
`reset --new-namespace` retires it under the same writer lock as the namespace
file. Retirement removes the mapping first and syncs the state directory before
removing the namespace and syncing again. An interruption between removals keeps
the old namespace usable with regenerated mappings; it cannot leave old mappings
behind a retired namespace. There is no reconciliation across retired namespaces. Custody export bundles
exclude this private operator state. Deleting it changes exported handles. Native IDs, log stems and source paths remain private.
Opaque evidence keys use the schema's UID shapes with a fixed `opaque/receiver`
prefix; they are handles, not native identity or filesystem authority.

`Snapshot::project` evaluates every retained contributor. Each exported session
is atomic, including its actually referenced dimensions. Denied nullable fields
are omitted; authorized null and zero remain distinct. A denied required field,
invalid value, row-existence/destination denial, missing dimension dependency or
unknown relation endpoint withholds the session. Parent/root relations add the
complete source cohort of the referenced session; withholding iterates until no
accepted relation points at a withheld session. This deliberately applies that
stricter cohort to the entire related session bundle. Unreferenced dimensions
never export; shared dimensions are projected separately in each session's cohort.
`dim_price` is withheld because local facts do not retain exact composite rate-row
identity; recorded costs and price versions still export. `fact_label` remains
refused. Coverage permission is separately required for the entire session because
this contract includes a contributing policy-version vector. No withheld counts,
versions or denominators enter the receiver envelope. Local diagnostic outcome
counts have no serializer and must stay local.

`Projection::serialize_receiver` builds fresh JSON with profile
`receiver_plain_v1`, session rows and opaque source/policy-version vectors. Nothing
is written automatically; callers own the returned bytes. Typed receiver names
and paths follow D7 under the intersection and existing credential checks. Source
paths are never emitted; permitted analytics target/repository paths are distinct
D7 fields. This profile must not be sent to Hub. Hub tokenization, objects,
isolated object admission, namespace claims/heads, withdrawals, transport and L1
remain deferred. This slice exposes a library only; no CLI/default-on behavior.
Pinning materializes the snapshot in memory and requires a momentarily available
shared writer lock; streaming/resource-budget admission is follow-up object work.

Provider/model/evidence-class egress requires explicit consent from both policy
layers for every contributor; matching provider/model entries intersect their
evidence classes rather than requiring identical whole consent objects. Complete
successful scans with zero reported findings are additionally required for a
receiver body-read decision. This crate
does not authenticate its inputs, read or transform a body, perform scanning,
store or audit authorization, create Hub tokens, enforce transport, or grant
offline access. #555 retains token/key generation; #594–596 retain channel,
cache, and replica behavior.
