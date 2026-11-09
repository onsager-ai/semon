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

Provider/model/evidence-class egress requires explicit consent from both policy
layers for every contributor; matching provider/model entries intersect their
evidence classes rather than requiring identical whole consent objects. Complete
successful scans with zero reported findings are additionally required for a
receiver body-read decision. This crate
does not authenticate its inputs, read or transform a body, perform scanning,
store or audit authorization, create Hub tokens, enforce transport, or grant
offline access. #555 retains token/key generation; #594–596 retain channel,
cache, and replica behavior.
