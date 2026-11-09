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
`serialize_governed_row` emits only its authorized typed fields; policy versions,
source IDs, scan state, detector/redactor versions, and coverage details stay in
private provenance/audit state and do not appear in serialization or debug output.

Receiver exports accept typed plain paths under the contributor intersection;
credential-bearing path values are refused. Hub accepts only `pt1_` path tokens,
and `body_read_allowed` is always false for Hub. Typed key/name fields reject
recognizable credentials and absolute-path laundering; the narrow metadata
validators do not infer that arbitrary short prose is safe.

`fact_label` is deliberately refused by this slice. The analytics schema describes
its intended categorical fields, but it does not yet provide a supported,
versioned field/value taxonomy registry. Until the L1 contract can evaluate an
approved categorical set, `field`, `value`, and `labeller` cannot be used to
export a free-text assertion. No raw `StepFacts`, native facts, goals,
explanations, prompts, evidence excerpts, or other bodies are serializable.

Coverage may be disclosed only through a later envelope that first checks
`coverage_allowed` across every contributor. When it is false, callers must
withhold the coverage row rather than export counts, source handles, policy or
scan versions, vector lengths, or denominators. Missing optional fields remain
withheld, while explicit `null`, unknown values, numeric zero, and empty strings
remain distinct when their typed field is authorized.

Provider/model/evidence-class egress requires explicit consent from both policy
layers for every contributor. Complete successful scans with zero reported
findings are additionally required for a receiver body-read decision. This crate
does not authenticate its inputs, read or transform a body, perform scanning,
store or audit authorization, create Hub tokens, enforce transport, or grant
offline access. #555 retains token/key generation; #594–596 retain channel,
cache, and replica behavior.
