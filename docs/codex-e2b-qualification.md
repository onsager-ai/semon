# Authenticated Codex on E2B qualification (#249)

Status: **not qualified**. This is the experiment procedure, not a passing
result or a production authentication decision. #250–#256 remain gated on
#249; Daytona #257 remains deferred.

## Reconciliation against main

Reviewed main `cd5b6a3` on 2026-10-03, together with #248–#257 and #10.
`semon-relay` already discovers Codex rollouts, handles rewrite generations,
restores verified original rollout bytes, and checks the target's OS writer
lock. It does not restore the workspace, authentication, or SQLite state.
[The existing spike](codex-resume-spike.md) qualifies a minimal synthetic
thread on CLI `0.159.0-alpha.3`, not authenticated model work.

The [Push protocol](mirror-protocol.md) already provides cursor restart and
receiver-prefix reconciliation. Its mirror contains redacted inputs and facts;
it is not a complete harness/workspace backup. Relay fencing fences streams,
not workspace writers. No Push WAL or replication redesign is justified by
the evidence available here.

Core lifecycle contracts belong in Semon. Durable embedding-server records,
inbox, migrations and APIs belong in semon-hub. Follow the issue dependencies:
#249 → #250 → #251 → #252 → #253 → #254 → #255 → #256; #257 follows the E2B
qualification. Do not substitute synthetic test success for an earlier gate.

## Prerequisites and ownership

Use a coordinator with configured E2B API access and a receiver outside the
guest. Pin and record the official E2B SDK version, sandbox template/build,
and unmodified Codex package version before creating compute. The local
`0.159.0-alpha.3` observation is a candidate pin, not an E2B measurement.
Choose an explicit guest model-authentication path supported by the deployment:
managed execution with scoped model authentication, or a user-operated personal
experiment where vendor terms and login support permit it. Record which was
chosen. A coordinator's existing ChatGPT login is not evidence of guest auth
or of suitability for a hosted multi-user product.

Provider credentials stay at the coordinator. Provision harness authentication
through the selected supported mechanism; never copy the coordinator's personal
Codex home, put secrets in fixtures/reports, or include auth files in ordinary
recovery manifests. Authentication availability after boot must be tested.

Give this experiment a unique ownership label and bounded lifetime. Record
creation intent before the provider call; reconcile an uncertain create using
that label before retrying. Cleanup may delete only experiment-owned resources,
after exporting evidence and preserving any needed recoverable original source.

## Procedure and required observations

1. Record three separate identities: an experiment logical-session identifier,
   the actual Codex thread ID from `thread.started`, and E2B sandbox ID. Retain
   the mapping at the coordinator. Record stable absolute workspace and
   `CODEX_HOME` paths, receiver machine/token binding, receiver URL, and guest
   Push cursor path. Keep the binding and paths unchanged after resume. This
   mapping is experiment bookkeeping, not an implemented durable runtime API.
2. Start existing `semon push --to URL --token-file PATH --watch` in the guest,
   against the independent reference receiver. Keep the token file mode 0600.
   Record Semon revision/binary hash, CLI/SDK/template versions, boot ID from
   `/proc/sys/kernel/random/boot_id`, and harness/Push process start times.
3. Run authenticated `codex exec --json` in an isolated Git workspace. Have it
   implement and test a small real task. Give it a fresh context marker in the
   prompt without asking it to save that marker to a workspace file. Require a
   committed file, an uncommitted tracked edit and an untracked file. Record
   command intent, thread receipt, turn outcome, exit status and stderr in
   protected original evidence. Do not retry uncertain prompt delivery.
4. At a completed turn, stop harness/workspace writers. Check no pending tools
   or permission requests remain. Capture Git HEAD/status and file hashes,
   original rollout path/bytes, relevant home paths and database inventory.
   Retain the guest filesystem; use SQLite backup APIs for any exported
   databases, never raw-copy live WAL files. Flush retained files before pause.
   Record the first receiver model and per-file complete-line coverage.
5. Request explicitly **filesystem-only** E2B pause, using the pinned SDK's
   documented operation. Record request, observed provider state and outcome.
   An unknown response requires inspection. A memory pause or deadline timeout
   does not establish this gate. Resume that sandbox and verify a different
   boot ID and absence of old harness/Push processes; PID reuse alone is not
   process continuity. Verify all recorded workspace hashes and paths before
   allowing a writer to start.
6. Restart Push manually with the same receiver binding and cursor location.
   Resume the recorded thread with `codex exec resume --json THREAD_ID` from
   the same workspace/home. Ask for the earlier context marker without
   supplying its value, and meaningful follow-up work on the earlier task.
   Record the actual resumed ID, provider/model success, command outcome and
   resulting file/test evidence. A successful exit alone is insufficient.
7. Test Linux writer exclusion separately: hold the target thread's exclusive
   `flock` and verify resume refuses the competing writer without modifying
   the rollout or performing work. Also exclude other workspace writers;
   this lock covers the thread, not every process that can edit the workspace.
   Record SQLite/path observations after resume, including whether the index
   still points at the retained absolute rollout path.
8. Compare original pre/post logs and final receiver bytes using Semon's
   redaction function for the expected mirror. Report complete-line coverage,
   missing and duplicate records, ordering and file hashes. Do not compare
   redacted mirror hashes directly with original-byte hashes. Read back both
   models and verify the same machine binding and Codex session ID, with the
   new turn attached to the existing session. Distinguish this identity result
   from the future durable logical-session record in #250.
9. Export protected evidence and a secret-free summary, then clean up owned
   compute. Report any incomplete cleanup explicitly. Attach the summary to
   #249 only as a passing milestone when all acceptance observations exist.

## Qualification limits and current blocker

Report the exact retained recovery set actually exercised. Retaining an entire
home proves only that set sufficient; it does not prove rollout-only recovery
minimal. Qualify a smaller set separately before deletion/reconstruction in
#255. Compaction, long/paginated histories, attachments, subagents, pending
tools, different CLI versions and non-Linux locks remain unsupported by this
experiment unless separately exercised. Reference-receiver success does not
prove authenticated hub readback or production ACK/storage durability.

On 2026-10-03 this worker's managed environment reported no configured secrets,
runtime variables or outbound identities; `E2B_API_KEY` was unset. Local Codex
reported `0.159.0-alpha.3` and a ChatGPT login, but no supported guest credential
path was established. No E2B sandbox was created, authenticated turn run,
pause/resume performed, or runtime identity/log result measured. The first gate
is blocked on E2B coordinator access and selection/provisioning of supported
guest model authentication. No implementation milestone is claimed complete.
