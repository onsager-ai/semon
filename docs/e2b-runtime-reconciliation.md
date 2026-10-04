# E2B launch reconciliation (#251)

`semon_runtime::e2b::reconcile_launch` plans from an authorized, persisted
logical session and an E2B resource inventory. It performs no network requests,
provisioning, credential resolution, guest bootstrap or lifecycle activation.
The provider adapter and hosted coordinator must enforce the following sequence.

1. Resolve the owner's workspace-scoped provider connection and recheck its
   usability. Load the current durable session, launch operation and epoch.
2. List resources using deployment/session/operation metadata. Include running
   and paused resources and consume every page with bounded request time and
   total work. A failed request, unknown state or pagination limit remains
   unknown; it cannot establish absence.
3. Reconcile. Exactly one fully matching resource may be adopted. Conflicting
   owner/workspace/epoch/version metadata, a different already-bound resource,
   or duplicates block adoption. Never delete resources to resolve duplication.
4. If an unbound Prepared operation has a complete empty inventory, the returned
   `ClaimCreate` requires an owner/revision/epoch CAS to InFlight. Only its winner
   may issue one create request, with the returned metadata and a qualified
   resource/lifetime/bootstrap profile. A losing controller reloads and inspects.
5. Persist the observed resource identity with the same authority checks before
   guest/bootstrap work. Model credential delivery and stable mirror enrollment
   are separate boundaries; provider metadata contains neither secrets nor input.

A crash after the InFlight commit, even before the request, leaves an uncertain
operation. An empty inventory does not authorize replay; inspect again or report
an explicit operator-visible blocker. Provider eventual consistency or a lost
response cannot create a second writable sandbox. A bound resource that is no
longer listed cannot be replaced by this planner. Qualified recovery and writer
exclusion are prerequisites for replacement; they are not inferred from absence.

## SDK evidence and remaining work

The official Python `e2b==2.52.0` wheel was inspected without credentials or
provider calls. `Sandbox.create` accepts metadata and a seconds-based timeout.
`Sandbox.list` defaults to both running and paused states and returns a paginator;
the adapter must iterate `next_items()` until `has_next` is false. Page size is not
a total inventory limit. Resource metadata must still be checked after filtering.
SDK source/docstrings are interface evidence, not account capability validation.

`scripts/e2b-inventory.py` is the read-only official-SDK worker. Its deployment
requires exactly `e2b==2.52.0`; it refuses a different/missing SDK. A private stdin
request has `version: 1`, a `scope` containing deployment/session/operation label
keys, and the coordinator-only `api_key`. Do not pass it in argv, log requests or
expose this protocol to browser/guest callers. The embedding supplies owner-scoped
credentials after authorization, closes stdin, caps stdout, and enforces a
25-second process deadline with cancellation/reaping. Inherited proxy and CA
settings are preserved. This is not yet wired to a public route or coordinator.

The worker returns only its fixed protocol: `complete` with a sanitized resource
list, `incomplete`, or `unavailable`. It makes no create/connect/pause/destroy or
guest call. It lists at most 32 pages/1,000 resources with a 20-second aggregate
deadline and five-second per-request timeout. Unknown states remain unknown;
reserved metadata is bounded, other vendor metadata and exception messages are
discarded. A late or partial result never authorizes create. Six SDK-interface
fixtures cover pagination, limits, failures, metadata and secret-free output.

The SDK exposes lifecycle settings with timeout action `kill` as the API's
documented default, and explicit filesystem-only timeout pause. Neither default
is a safe application checkpoint policy. The qualified hosted profile must
establish bounded lifetime behavior without deleting uncheckpointed accepted
work or activating automatic pause. No such profile is enabled by this module.

Tests cover reload after uncertain create, eventual visibility, inaccessible or
incomplete inventories, paused adoption, conflicting metadata, duplicate
resources, foreign deployments, bound-resource loss and withdrawn launch intent.
Process transport integration, account/default-profile validation, bootstrap, private
guest credential delivery/writeback, pause/resume and real Hub ingestion/readback
remain implementation and qualification work. Restore, export and deletion stay
behind their independent gates; automatic policy remains Off.
