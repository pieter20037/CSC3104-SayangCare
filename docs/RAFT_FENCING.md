# Volunteer ownership fencing: operation and validation

This enhancement separates Redis waiting order from optional Raft ownership
coordination. PostgreSQL enforces fencing in both configurations.
See the [original proposal](RAFT_FENCING_PROPOSAL.md) for motivation and sources.

## What changed

- Claim, transfer/reassignment and release/resolve advance a retained per-case
  `assignment_generation`: claim 1, transfer 2, release 3, reclaim 4.
- PostgreSQL validates current owner and generation while holding the same
  transaction lock used for protected changes.
- Notes, transfer, release and resolve require explicit generations.
- Ownership changes, assignment history and session checkpoints install in
  one PostgreSQL transaction.
- A durable operation journal retains stable request IDs. Ambiguous Raft
  outcomes stay pending and are retried by background recovery.
- The dashboard displays generations and sends its displayed token with each
  action. A stale browser cannot silently acquire a newer token and resubmit.
- Redis still orders waiting callers and holds best-effort projections.
- Kubernetes HPA replicas do not become Raft voters.

This is not authentication. Role tabs and volunteer IDs remain prototype
identity selectors. Use synthetic data only.

The fencing revision has also been rebuilt and evaluated in Kubernetes.
The [current evaluation](PHASE6_Windows.md) records 113 workspace passes,
passing baseline/API-loss/Redis/HPA runs and the remaining PostgreSQL
availability limitation. Those Kubernetes pods use PostgreSQL ownership,
not embedded Raft voters. The isolated Raft results below remain a separate
experiment, not evidence that Kubernetes HPA operates a Raft cluster.

The [later PostgreSQL recovery follow-up](PHASE6_Windows.md#postgresql-recovery-follow-up-3-november-chews-local-cluster)
passed one strict primary-replacement scenario after primary-aware pool
validation, clean fast shutdown and bounded idempotent session creation.
It does not add automatic retries to ownership mutations, alter fencing, or
revalidate the separate Raft experiments.

## Upgrade safely

Build and deploy current code using the existing deployment guides. The API
runs migration `0011_assignment_fencing.sql` during bootstrap.

Back up valuable data first. The additive migration retains assignment history,
backfills generations, creates ownership/journal/notes tables, and updates
checkpoint generations. Do not edit an already applied migration or delete
database/Raft volumes to make an upgrade appear successful.

Legacy active Raft assignments do not contain generations. Startup refuses
to enable fenced coordination over these records without offline reconciliation.
Likewise, a session's retained PostgreSQL generation cannot silently switch
between PostgreSQL-only and Raft authority. No automatic import or online
backend-switching protocol is provided in this milestone.

Use fresh synthetic cases for the comparison. If a legacy deployment is
refused, preserve its data and plan an explicit offline migration. The existing
Kubernetes/PostgreSQL mode remains available for its own data.

## Two configurations

| Mode | Queue ordering | Ownership coordinator | Application URL |
| --- | --- | --- | --- |
| Existing Kubernetes default | Redis | PostgreSQL transaction | `http://127.0.0.1:18080` |
| Docker Raft configuration | Redis | Fixed 3-member OpenRaft cluster | Ports 18081-18083 |

Start Docker Desktop first. From the repository root:

```powershell
.\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
```

In another terminal, for the already installed Kubernetes deployment:

```powershell
.\scripts\start-dashboard.ps1 -Mode Kubernetes -OpenBrowser
```

These are separate deployments/databases. Starting both does not join their
state. The launcher must run the newly built image to expose the new endpoints;
old running images do not change just because source files changed.

## Dashboard demonstration

1. Use a fresh synthetic escalation in Caller lab.
2. Select an on-shift volunteer in Volunteer desk and claim your case.
3. Show generation 1 in the assigned case and session.
4. Open another tab showing that assignment; retain it as the stale view.
5. In the first tab, transfer to another on-shift volunteer. Show generation 2.
6. Submit a note/action using generation 1 in the stale view or HTTP example.
   Expect HTTP 409, not an accepted update.
7. With the new owner's generation 2, enter text in **Synthetic case note**,
   then click **Add case note** on the intended case.
8. Release to queue: generation 3, no owner. Claim again: generation 4.

Auto-refresh may update browser views. The HTTP example is the reliable way to
demonstrate a deliberately retained old token.

The supplied roster has Aisha and Noor on shift; Ibrahim is initially off shift.
The Raft panel shows leader, term and applied log indexes; the status endpoint
also exposes local ownership snapshots for catch-up checks. Diagnostic follower
snapshots can lag and are not an authoritative read protocol.

## HTTP contract

```text
POST /api/v1/volunteers/claim
  {session_id, volunteer_id, request_id, simulate: true}

POST /api/v1/volunteers/{volunteer}/cases/{sid}/transfer
  {assignment_generation, request_id, to_volunteer_id?}

POST /api/v1/volunteers/{volunteer}/cases/{sid}/release
POST /api/v1/volunteers/{volunteer}/cases/{sid}/resolve
  {assignment_generation, request_id}

POST /api/v1/volunteers/{volunteer}/cases/{sid}/notes
  {assignment_generation, request_id, body}

GET /api/v1/calls/{sid}/ownership
GET /api/v1/calls/{sid}/ownership/audit
```

Use a unique request ID for each distinct command. Retry an uncertain operation
with its original request ID and payload. Reusing an ID with different content
is a conflict. For Claim next, the journal retains the originally selected case.

Claim accepts a missing request ID for existing callers, but clients needing
reliable retries must supply one themselves. Transfer/release/resolve require
both generation and request ID; missing fields return HTTP 400.

Stale generations return HTTP 409:

```json
{"error":"Stale assignment generation (fencing token rejected)"}
```

Unavailable coordination or pending ownership returns HTTP 503. It does not
mean an operation definitely did not commit. Inspect ownership/pending state
and retry the original operation. Do not issue a replacement command with a
new request ID to override pending state.

The audit endpoint returns database-backed assignment history and note request
IDs/generations, without note text. It is an application-exposed SQL audit,
not independent administrator SQL execution.

## Recovery protocol

```text
Persist pending intent in PostgreSQL
    -> commit expected-generation transition through Raft
    -> atomically install ownership/history/checkpoint in PostgreSQL
    -> acknowledge and reconcile Redis
```

PostgreSQL-only mode performs the ownership transition and installation in one
transaction. It does not need the intermediate quorum round trip.

During a pending transition, protected notes cannot write through the old
assignment. Recovery reuses the journaled request ID; it never decrements a
generation or silently switches Raft mode to PostgreSQL-only.

Background recovery runs on the existing handoff maintenance interval.
Database unavailability still blocks ownership installation. Quorum loss
blocks new Raft ownership decisions, but does not necessarily block a note
from an installed owner on an unrelated case with no pending transition.

Redis changes are recoverable projections rather than transaction participants.
External phone redirects are also not part of the PostgreSQL transaction.
They run after accepted ownership; failures are reported for operator review.
A replay does not promise exactly-once phone side effects.
An intent recovered after a lost response does not guarantee phone dispatch or
redirect delivery. Operators must inspect ownership and review the live call;
this milestone does not implement a transactional telephony outbox.

## Automated validation

```powershell
cargo test --workspace --locked
```

The workspace includes deterministic state-machine checks and an in-process
three-node OpenRaft failure/rejoin test. They do not prove physical-machine
or arbitrary network-partition tolerance.

Database-backed tests are explicitly ignored by default. Configure private
`TEST_DATABASE_URL` and `TEST_REDIS_URL` for dedicated test infrastructure,
then run:

```powershell
cargo test -p sayangcare-session-store --locked transactional_fencing_and_ambiguous_commit_recovery -- --ignored
cargo test -p sayangcare-api --locked http_claim_race_transfer_notes_and_stale_owner_are_fenced -- --ignored
```

The tests create isolated schemas. Use a disposable test database and Redis,
not production data. Never paste passwords into the report or screenshots.

The storage test checks concurrent ownership, stale notes/transfers/releases,
release/reclaim, A-to-B-to-A fencing, lost coordinator response, and a database
installation failure after commitment. The API test checks actual HTTP
200/409 behaviour and persisted notes.

## Comparison and controlled faults

```powershell
.\scripts\test-raft-fencing.ps1
```

Use the script's parameter help for sample counts, custom URLs and explicit
fault injection:

```powershell
Get-Help .\scripts\test-raft-fencing.ps1 -Detailed
```

Fault injection is opt-in. Tell teammates first; it stops identified Raft
containers and restores only the ones it stopped. Never remove volumes.
Check the generated JSON's assertions and restoration result, not only the
console summary.

Report median/p95 by operation type and count failures separately. Raft leader
and quorum scenarios are not applicable to PostgreSQL-only mode. Docker versus
Kubernetes has different databases/resources, so it is a deployment comparison,
not isolated Raft-overhead measurement.

Retain unsuccessful runs. Historical Phase 5 JSON does not validate this new
generation protocol. Record actual test commands/counts and distinguish
in-process tests, HTTP smoke tests and Docker fault experiments.

## Recorded validation: 3 November 2026

| Validation | Observed result |
| --- | --- |
| Workspace baseline before implementation | 105 passed, 5 ignored |
| Updated `cargo test --workspace --locked` | 112 passed, 7 ignored; no failures |
| Final `cargo build -p sayangcare-api --locked` | Passed, including updated embedded dashboard; existing warnings retained |
| Explicit PostgreSQL fencing integration test | Passed: race, stale mutations, ABA, lost response, post-commit installation failure and recovery |
| Explicit HTTP integration test | Passed: mandatory fields, claim race, transfer, notes, release and stale-owner rejection |
| Existing durable checkpoint integration test | Passed separately |
| Three-node in-process failure/rejoin | Passed, including repeated runs |
| Comparison harness static/mock validation | Passed |
| Real HTTP comparison, both modes | Correctness checks passed; 1 warmup and 3 measured samples per operation |
| Real API process leader stop | Leader 1/term 1 became leader 2/term 2; transfer succeeded at generation 2 |
| Real API process quorum loss | New transfer and fresh claim returned 503; pending-case note returned 503 |
| Process restart/catch-up | All three reached leader 2, term 2, applied index 41; pending transfer/claim recovered |
| Independent administrator SQL | Raft: 8 cases; PostgreSQL-only: 7 cases; neither had pending or unfinished operations |
| Browser, current dashboard source | Claim 1, transfer 2, note 2, release 3, reclaim 4, resolve 5; no final workspace error |

Both deployments used isolated host API processes with temporary PostgreSQL
and Redis containers. The harness fault switch was **not** used on the team's
Docker stack. Leader/quorum faults were separately orchestrated against owned
host processes, not the existing containers or Minikube cluster.

The browser used a test proxy serving current HTML to the PostgreSQL-only API.
This verified source UI behaviour, not a rebuilt deployed dashboard image.

### Latency smoke results (milliseconds)

| Operation | Raft median / p95 | PostgreSQL-only median / p95 |
| --- | --- | --- |
| Claim | 81.30 / 82.57 | 59.94 / 61.30 |
| Transfer | 72.49 / 81.48 | 50.54 / 51.24 |
| Note | 14.74 / 15.59 | 10.70 / 13.85 |
| Release | 67.71 / 81.49 | 45.85 / 48.43 |

Only three measured samples per operation: these numbers do not establish
production latency, capacity, or statistically reliable Raft overhead.
Both modes reject stale owners; Raft is not uniquely required for fencing.
The approximately eight-second quorum errors reflect the bounded unknown-outcome
wait, not successful ownership decisions or an availability guarantee.

### Evidence files

- [Machine-generated comparison report](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json)
- [Consolidated final build/test results](../artifacts/fencing/20261103T063451737Z-3f552c29/final-verification.json)
- [Comparison HTTP requests](../artifacts/fencing/20261103T063451737Z-3f552c29/requests.jsonl)
- [Manually consolidated live-process/browser observations](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json)
- [Restored node diagnostic snapshots](../artifacts/fencing/20261103T063451737Z-3f552c29/restored-node-status.json)
- [Independent Raft database SQL audit](../artifacts/fencing/20261103T063451737Z-3f552c29/fencing_live_raft-sql-audit.json)
- [Independent PostgreSQL-only SQL audit](../artifacts/fencing/20261103T063451737Z-3f552c29/fencing_live_pg-sql-audit.json)

The SQL snapshots were collected after the separate fault and browser checks,
so their case counts exceed the earlier comparison report's counts. The
comparison report correctly leaves its own Docker-fault and administrator-SQL
checks unexecuted; supplementary files do not retroactively change that run.
This revision has not been rebenchmarked on Kubernetes or validated with live
Twilio calls. Preserve those distinctions in the final report.

## Remaining boundaries

- No automatic conversion of populated legacy Raft assignment state.
- No dynamic Raft membership or Raft integration into Kubernetes HPA.
- No off-host failure/backup-restore certification.
- Durable idempotency journals, Raft outcomes and ownership tombstones need an
  explicit future retention policy; do not prune them casually.
- Existing PostgreSQL availability/pool-capacity limits still apply.
- Full cross-store linearizability and exactly-once external effects are not
  claimed merely because a generation counter and Raft are present.
