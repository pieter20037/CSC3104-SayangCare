# Final Milestone Proposal: Majority-Backed Ownership and Fencing

SayangCare | Proposed 3 November 2026

**Status:** implementation proposal, not a completed enhancement or test result.
The repository was inspected to prepare this design. No new fencing,
failover or benchmark results are claimed here.

Implementation follow-up: see [the operation and validation guide](RAFT_FENCING.md).
This document preserves the pre-edit design; the guide and recorded evidence
distinguish implemented behaviour from experiments actually executed.

## 1. Executive summary for the professor

We propose narrowing OpenRaft's responsibility to volunteer ownership
transitions: claim, transfer/reassignment and release. Redis continues to order
waiting callers. PostgreSQL retains authoritative session checkpoints,
assignment history and protected case data.

Every successful ownership transition advances a per-case
`assignment_generation`. A request from a previous owner must present its old
generation and be rejected atomically by PostgreSQL before it changes case data.
Three Raft members commit ownership decisions through a majority of two.

The comparison implementation uses PostgreSQL transactions to provide the
same fencing property without Raft. We will measure correctness, availability,
recovery and latency in both configurations. We do not claim that PostgreSQL
cannot solve ownership coordination, or that Raft is inherently necessary for
this application.

**Research question:** what safety and availability behaviour does an explicit
majority-backed ownership coordinator provide, and what operational and latency
cost does it add relative to PostgreSQL-only coordination?

## 2. Motivation and bounded scope

Kubernetes replaces failed compute instances, Redis orders waiting work, and
PostgreSQL already stores durable conversation and assignment records.
Presenting Raft merely as another queue recovery mechanism does not clearly
differentiate its responsibility.

The proposed property is more specific:

> Once reassignment is acknowledged, no protected request carrying a previous
> assignment generation may modify the case.

For example:

1. A claims a case and receives generation 1.
2. The case transfers to B, who receives generation 2.
3. A delayed request from A arrives carrying generation 1.
4. PostgreSQL rejects it with HTTP 409 without applying the mutation.
5. B's equivalent request carrying generation 2 succeeds.

This demonstration concerns ordering and fencing, not clinical safety,
authentication, transcript recovery or exactly-once execution of all HTTP calls.

### Existing foundation versus proposed work

The current [Compose configuration](../docker-compose.raft.yml) already selects
Redis queue ordering and enables Raft assignment coordination independently.
[Application state](../crates/api/src/state.rs) supports this configuration.
The [Raft state machine](../crates/priority-queue/src/raft_state.rs) already has
assignment claim, transfer and release commands, but stores an owner without
a generation and removes assignment state on release.

The [volunteer handlers](../crates/api/src/handlers/volunteer.rs) and
[binding store](../crates/session-store/src/volunteer.rs) currently perform
multiple storage/coordinator operations. Adding a token field alone would not
make these operations one transaction or recover ambiguous outcomes.

This milestone adds generation-aware coordination, storage enforcement and
reproducible evidence. It does not rebuild the calling, inference, monitoring
or Kubernetes components.

## 3. Separation of responsibilities

| Component | Responsibility | Explicit exclusion |
| --- | --- | --- |
| Redis | Waiting order: risk descending, then FIFO; best-effort projections | Not the authority for assignment generations |
| OpenRaft, when enabled | Commit ownership transitions and their generations | Not session storage or Kubernetes orchestration |
| PostgreSQL | Enforce generations; store ownership projection, assignment history, session checkpoints and protected mutations | Does not become available merely because Raft has quorum |
| PostgreSQL-only coordinator | Transactional ownership and generation allocation in comparison/default mode | No Raft quorum or leader election |
| Kubernetes | API routing, replacement and scaling in the existing deployment | Does not reconstruct lost process memory or issue fencing tokens |

```text
Waiting callers
      |
      v
Redis priority order
      |
      v
API ownership service
      |
      +-- Raft mode: commit transition through 2/3 members
      |
      +-- PostgreSQL-only mode: commit transition transactionally
      |
      v
PostgreSQL: owner + generation + assignment history + protected case changes
      |
      v
Dashboard: owner, generation, transition outcome
```

Redis selection is advisory until ownership is committed. A lost/stale queue
entry must not override current PostgreSQL ownership. Existing stale-entry
skipping remains; projection/queue inconsistencies require explicit recovery.
We do not claim a transaction spanning Redis, Raft and PostgreSQL.

Raft remains a fixed three-member Docker deployment on ports 18081-18083.
The existing Kubernetes API deployment on port 18080 remains PostgreSQL-only
for ownership unless explicitly configured otherwise. No voters will be added
to dynamically autoscaled HPA pods.

## 4. Proposed structures and generation rules

```rust
pub struct AssignmentOwnership {
    pub volunteer_id: Option<String>,
    pub assignment_generation: u64,
}

pub enum OwnershipTransition {
    Claim { volunteer_id: String },
    Transfer {
        from_volunteer_id: String,
        to_volunteer_id: String,
    },
    Release { volunteer_id: String },
}

pub struct OwnershipCommand {
    pub request_id: String,
    pub session_id: SessionId,
    pub expected_generation: u64,
    pub transition: OwnershipTransition,
}
```

The deterministic outcome distinguishes applied, stale generation, ownership
conflict and generation exhaustion. Applied outcomes include the resulting
owner/generation. Raft replies may additionally expose the committed log index;
that index is not substituted for the per-case generation.

| Transition | Result |
| --- | --- |
| Unassigned, never claimed | Generation 0, no owner |
| A claims | Generation 1, owner A |
| A transfers to B | Generation 2, owner B |
| B releases/resolves | Generation 3, no owner |
| C claims again | Generation 4, owner C |
| Rejected/stale command | No generation increment |
| Replay of the same committed request | Same outcome, no additional increment |

Release retains an ownership tombstone; it must not delete/reset the counter.
Counters use checked increments. PostgreSQL `BIGINT` is signed, so accepted
values are bounded by `i64::MAX` and converted without unchecked casts.

An ownership-coordinator trait will expose typed transition outcomes to the
API. Raft and PostgreSQL-only implementations share the handler-facing contract.
The existing priority-queue trait continues to represent waiting order.

## 5. Proposed additive PostgreSQL migration

Use the repository's existing `session_id` identifier rather than introducing
a second `call_sid` column for the same concept.

| Table | Proposed data |
| --- | --- |
| `volunteer_assignments` | Add positive `assignment_generation BIGINT NOT NULL DEFAULT 1` to history |
| `case_ownership` | One row per session: current nullable owner, retained nonnegative generation, pending-operation identifier, timestamps |
| `assignment_operations` | Stable request ID, session, expected generation, transition, immutable payload and pending/completed outcome |
| `volunteer_case_notes` | Session, author, accepted generation, note text and timestamp for the protected-write demonstration |

The durable operation journal is needed for recovery and idempotent API retries;
it is not a replacement for the existing assignment history.

Existing assignment history will be backfilled deterministically in per-case
order, retaining all rows and deriving current ownership from active/history
records. The migration must validate contradictory ownership instead of
silently choosing an owner.

Existing Raft snapshots contain a different assignment representation.
Upgrade logic must support that representation and verify agreement with
PostgreSQL before enabling fenced writes. A nonempty mismatch requires an
explicit, recoverable bootstrap/import procedure with ownership writes paused.
It must not reset generations, delete volumes or silently overwrite either
store. Backend switching on an existing dataset is not a runtime failover
mechanism.

## 6. Atomic fencing and partial-failure protocol

### 6.1 Protected writes

Transfer, release/resolve and notes must carry a generation. PostgreSQL checks
all of the following inside the same transaction as the mutation:

- The session's current generation equals the supplied generation.
- The current owner equals the requesting volunteer.
- There is no unresolved ownership transition.
- The relevant assignment is active.

The current-ownership row is locked/conditionally updated so validation cannot
race a concurrent transfer. Checking the token in application memory and then
saving separately is insufficient.

For example, the guard has this shape, with the lock held through the write:

```sql
SELECT session_id
FROM case_ownership
WHERE session_id = $1
  AND assignment_generation = $2
  AND volunteer_id = $3
  AND pending_operation_id IS NULL
FOR UPDATE;
```

For protected session-status changes, the generation guard and checkpoint
version check must participate in the same PostgreSQL transaction. Assignment
history and ownership updates must not commit independently of that change.

No matching current generation returns HTTP 409:

```json
{"error":"Stale assignment generation (fencing token rejected)"}
```

Token equality is not authentication. Existing prototype identity selectors
remain a separate production-security limitation.

### 6.2 Ownership transitions in Raft mode

1. Validate inputs and atomically reserve a per-case transition in PostgreSQL,
   retaining its immutable payload and stable request ID in the journal.
2. Mark the case pending. Reject/block conflicting protected mutations and
   ownership transitions while this operation is unresolved.
3. Submit the deterministic transition to Raft using the same request ID and
   expected generation. Raft checks ownership/generation in committed order.
4. After commitment, atomically install the result in PostgreSQL: current
   ownership/generation, assignment history, checkpoint changes and journal
   completion.
5. Return success only after PostgreSQL installation. Update/reconcile Redis
   projections separately and report/log failures explicitly.

No PostgreSQL transaction or connection should be held open while waiting
for a quorum. Pending state is durable across that wait.

The reservation is important: once an ownership transition is committed in
Raft but not yet reflected in PostgreSQL, the database must not continue
accepting stale protected writes through the old assignment.

### 6.3 Failure and recovery rules

| Failure point | Required behaviour |
| --- | --- |
| Before a durable reservation | No ownership transition has been submitted |
| Quorum unavailable or result unknown | Return 503; retain pending intent; do not assume timeout means abort |
| Raft committed, PostgreSQL installation failed | Return non-success; block conflicting writes; recover the same operation |
| Database installed, response lost | Retry by request ID returns the recorded outcome without advancing again |
| Redis projection failed | Do not reverse ownership; log and reconcile projection |
| API restarted | Resume pending journal operations using the original request IDs |

A recovery worker resubmits/reconciles the same transition and completes its
database installation. A timeout is not permission to clear pending state:
the transition may already be committed or may commit later.

Do not compensate a failed later step by restoring an earlier generation.
Reversal, if required, is a new committed ownership transition.
Idempotency retention and payload matching must be explicit; reusing one
request ID with different content is rejected.

### 6.4 PostgreSQL-only mode

Use the same guard, journal, generation rules and HTTP response contract.
Ownership, assignment history and protected session updates commit in one
PostgreSQL transaction, without a Raft round trip.

Raft mode never silently switches to this implementation during quorum loss.
Such a fallback would bypass the configured ownership authority.

### 6.5 Availability boundary

Ownership changes require both Raft quorum and PostgreSQL availability in Raft
mode. A 503 may represent an operation with an uncertain eventual outcome;
clients retry with the same request ID or inspect its status.

Existing-owner notes may continue without a new Raft command if PostgreSQL has
an installed active generation and no pending transition. Once a transition
is pending, they fail closed. Therefore "quorum loss" does not mean every
application endpoint must return 503.

External effects such as phone redirects are not transactionally fenced by
PostgreSQL. They must occur after accepted ownership, with explicit failure
reporting; no exactly-once external-call guarantee is claimed.

## 7. API and dashboard changes

- Claim responses expose `assignment_generation` and the operation request ID.
- Transfer, release/resolve and notes require `assignment_generation`; omission
  is a validation error, never an implicit use of the latest generation.
- Claiming an unowned case does not require the user to guess its next token;
  the coordinator allocates it using the reserved current generation.
- Optional transfer target permits a deterministic A-to-B demonstration;
  existing automatic-target selection can remain when no target is supplied.
- Add one minimal notes endpoint as the concrete protected update.
- Explicit release returns a case to waiting eligibility where appropriate;
  resolve closes the handoff. Both revoke ownership and increment generation.
- Assigned-case/session views display the generation and pending state.
- Dashboard buttons submit the displayed token rather than fetching a newer
  token to silently replay a stale action.
- Diagnostic Raft status exposes applied ownership generations for catch-up
  evidence. A follower's diagnostic snapshot is not an authoritative read.
- Background release/cleanup follows the same coordinator and fencing rules;
  it cannot bypass them through the binding store.

Existing unfenced transfer/resolve clients must be updated. This is an
intentional API tightening, not a silently backward-compatible change.

## 8. Required acceptance tests

Run each safety test with explicit assertions, not screenshots alone.

| ID | Scenario | Passing evidence |
| --- | --- | --- |
| FENCE-01 | Two nodes concurrently claim the same fresh case for A and B | Exactly one claim succeeds with generation 1; the loser returns 409; one active durable assignment |
| FENCE-02 | Claim A, transfer to B, replay A's delayed note/transfer/release | Claim 1; transfer 2; all stale actions return 409 and change no protected data; B's generation-2 note returns 200 |
| FENCE-03 | Stop current leader with two members available | A new leader is observed in a higher term; a new ownership transition commits and advances the generation |
| FENCE-04 | Stop two of three members | New claims/transfers return bounded 503/unavailable; no success or PostgreSQL-only fallback; pending outcomes are reconciled after restoration |
| FENCE-05 | Restart stopped members | All three converge on the ownership state and current leader; each applies the target transition's log index within the declared timeout |

A continually advancing Raft log need not have identical sampled indexes at
every instant. Catch-up means each member has applied the target committed
operation, with the same resulting ownership generation, and converges after
the test becomes quiescent.

Additional required regressions:

- Release/reclaim increments and retains the counter.
- Replay of the same request has one effect; conflicting payload reuse fails.
- A committed transition followed by database failure recovers after restart
  without accepting old-generation writes in the gap.
- Old-owner A cannot act after an A-to-B-to-A cycle using A's first token.
- Counter exhaustion, missing tokens and out-of-range values are rejected.
- Existing stale queue entry skipping and session/archive recovery remain.
- Legacy snapshot/history migration preserves monotonicity or refuses unsafe
  activation explicitly.

Use deterministic state-machine tests, the existing in-process three-node
OpenRaft harness and PostgreSQL-backed API integration tests. Then run the
Docker HTTP/failure script. In-process network isolation is not proof of
physical-machine failure tolerance.

The existing workspace suite must pass alongside the new tests. The historical
100-passing-test figure is a reference, not a newly verified current baseline.
Establish and record actual before/after counts; ignored tests are not passes.

## 9. Reproducible comparison script and evidence

Create `scripts/test-raft-fencing.ps1` with:

- Explicit URLs: Raft nodes 18081-18083 and comparison API 18080.
- Preflight checks for actual coordinator mode, healthy storage and all Raft
  members. Reject accidental comparisons against the wrong deployment.
- Unique synthetic session/request IDs and a start barrier for concurrent
  requests. "Concurrent" does not promise identical physical arrival times.
- A latency-only mode and an explicitly confirmed fault-injection mode.
- Bounded HTTP/election/recovery timeouts and clear failed assertions.
- Exact Docker Compose project/service targeting; never stop unrelated
  containers or delete volumes.
- `finally` restoration of only containers stopped by this run, followed by
  readiness/catch-up verification. Restoration failure must remain visible.
- JSON evidence, request/status timings, SQL audit results, summary table and
  nonzero exit on failed acceptance.

| Measurement | With Raft | Without Raft |
| --- | --- | --- |
| Concurrent single-owner correctness | Test | Test |
| Stale-generation rejection | Test | Test |
| Median/p95 successful transition latency | Measure by operation type | Measure by operation type |
| Failed/time-out requests | Count separately | Count separately |
| Raft leader failure and election time | Test | Not applicable |
| Raft quorum loss | Ownership changes fail closed | Not applicable |
| API replica failure | Separate controlled scenario | Separate controlled scenario |
| Database unavailability | Report dependency failure | Report dependency failure |

Predeclare latency sample count, warm-up, concurrency, timeout and fault timing.
Do not pool different operation types into one misleading percentile or drop
failed requests from the availability denominator.

Prefer matched environments for an algorithm-overhead comparison. Comparing
Docker Raft plus a single Docker PostgreSQL service against Minikube plus
replicated PostgreSQL is a deployment comparison with confounding variables,
not an isolated measurement of Raft cost. Record that limitation if using
the existing ports/topologies.

An evidence report must include:

- Run ID, UTC timestamps, Git revision, images and configuration.
- Coordinator mode, topology and database replication settings.
- Scenario assertions and expected/observed status codes.
- Ownership generations and accepted/rejected mutation records.
- Leader, term, target applied index and restoration/catch-up observations.
- Operation latency samples, median/p95, failure counts and recovery intervals.
- PostgreSQL audit of active ownership, history and protected notes.
- Native process/script exit and any unresolved operation IDs.

No result values will be filled in until the experiment is executed.
Prior session-loss, Redis-failover and Kubernetes HPA results do not validate
this new ownership protocol.

## 10. Implementation sequence and completion gate

1. Confirm this protocol and record the current workspace-test baseline.
2. Add generation state/commands and deterministic tests.
3. Add migration, transactional fencing and operation-journal recovery.
4. Wire both coordinator implementations, all protected handlers and cleanup.
5. Update dashboard tokens/visibility and error handling.
6. Add API/database/three-node tests, then run the comparison script.
7. Publish actual evidence and update runbooks/report claims.

**Complete only when:** workspace regressions pass; all five scenarios and
partial-failure recovery have explicit evidence; both modes enforce the same
stale-owner property; stopped services are restored; and limitations accompany
the comparison results.

No live Twilio exercise, Raft-on-Kubernetes redesign, new multi-volunteer
workflow or change to AI/session durability is part of this milestone.

## 11. Justification and references

PostgreSQL can already issue generations and enforce atomic ownership.
Raft is therefore an explicit design alternative to evaluate, not a necessary
condition for fencing. Its contribution is a replicated, majority-backed
ownership decision history; its costs include another availability dependency,
cross-system recovery and operational complexity.

Raft safety depends on correct implementation, persistence and recovery under
its failure model. Three processes and a leader panel alone do not prove
application-wide linearizability. Diagnostic follower reads can be stale.

1. Diego Ongaro and John Ousterhout, *In Search of an Understandable Consensus
   Algorithm*, 2014. [Raft project and paper](https://raft.github.io/).
   Supports the replicated-state-machine and majority-commit model, not an
   automatic proof of this application's cross-store protocol.
2. Martin Kleppmann, *How to do distributed locking*, 2016.
   [Article](https://martin.kleppmann.com/2016/02/08/how-to-do-distributed-locking.html).
   Explains stale-client hazards and why the protected resource must enforce
   fencing tokens. It does not establish that PostgreSQL-only fencing is
   inadequate or that all uses of Redis are unsafe.
3. Jepsen, *Redis-Raft 1b3fbf6*.
   [Analysis](https://jepsen.io/analyses/redis-raft-1b3fbf6).
   Illustrates the need to test real consensus implementations and failure
   behaviour. Findings concern particular Redis-Raft development builds, not
   evidence that OpenRaft or this application is correct.

### Short consultation/submission statement

> We propose majority-backed ownership coordination with per-case fencing
> generations, not another session-recovery mechanism. Redis retains waiting
> order; optional OpenRaft commits ownership transitions; PostgreSQL atomically
> rejects stale-owner updates and stores assignment history. A durable pending
> operation protocol handles the gap between Raft commitment and database
> installation. We will compare this with PostgreSQL-only coordination using
> identical safety assertions and report latency, recovery and availability
> honestly. Implementation and measurements are the final milestone; this
> document does not claim that those new tests have passed.
