# Governance decision library

`semon-asi::governance` is the deterministic local decision layer for the #592
policy slice. Policies use strict format 1 JSON. Unknown fields and unsupported
versions fail closed. `PolicyBundle::content_version` hashes canonical typed
baseline and local policy content; caller-supplied version strings are not
accepted. Source floors and the stricter of baseline/local restrictions apply
before a decision is returned.

`evaluate` requires a nonempty, unique contributor set with explicit source
identity, owner, namespace, machine, classification floor and detector/redactor
evidence. Policy scope is destination + namespace + machine, and both baseline
and local policy must allow the authenticated principal or one of its groups.
The caller must supply a trusted principal and the complete admitted provenance.
This crate does not authenticate either value. Unknown source membership or a
missing policy returns an error.

Successful derived-row projection requires every contributor to allow row
existence and every included field. It returns the per-source policy-version
vector with the typed fields. A missing optional field is withheld; explicit
`null`, unknown state, numeric zero and empty text remain distinct values. Denied
required fields, body fields, unknown columns and invalid values reject the row.
Coverage disclosure requires unanimous `allow_coverage`. Callers must withhold
the entire row when projection fails; they must not remove a contributor and
publish a partial total.

Receiver projection permits schema-typed paths and allowlisted short names. Hub
projection accepts only `pt1_` path tokens and allowlisted names. Token creation,
key custody and CLI behavior remain #555 work. This library validates records;
it does not read or transform a body, perform a scan, store/audit/cache an
authorization, enforce a transport, or grant offline body access. `ScanEvidence`
records known scan state, detector/redactor versions and findings. A completed
scan with no findings is still not a claim that detection is perfect. Body reads
require complete scans and zero reported findings in addition to unanimous
policy consent.

Provider/model/evidence-class egress requires explicit consent in both policies
for every contributor. Analytics and body-read permission do not imply model
consent. No provider is called by this crate. Size, request and TTL values are
intersected as restrictions for future transport callers; #594–596 channel,
cache and replica behavior is outside this API.
