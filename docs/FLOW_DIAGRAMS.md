# Implementation Flow Diagrams

## Circuit Breaker State Machine (CB-01, CB-02)

### Normal Flow: Closed → Open → Half-Open → Closed

```
┌──────────────────────────────────────────────────────────────────┐
│                          CLOSED STATE                            │
│  - Normal requests allowed                                       │
│  - Metrics collected (latency, errors)                          │
│  - permit = breaker.acquire() → RequestPermit::Normal            │
└──────────────────────────────────────────────────────────────────┘
            ↓
        Error Rate OR P95 Latency exceeds threshold
        (based on sentiment-aware adaptive policy)
            ↓
┌──────────────────────────────────────────────────────────────────┐
│                          OPEN STATE                              │
│  - All requests rejected with CircuitOpen error                  │
│  - Cooldown timer starts (e.g., 60 seconds)                      │
│  - Generation incremented (invalidates old requests)             │
│  - breaker.acquire() → Err(CircuitOpen)                          │
└──────────────────────────────────────────────────────────────────┘
            ↓
        Cooldown elapsed (tick() called)
            ↓
┌──────────────────────────────────────────────────────────────────┐
│                        HALF-OPEN STATE                           │
│  - ONE request permitted (recovery probe)                        │
│  - permit = breaker.acquire() → RequestPermit::RecoveryProbe     │
│  - Other requests rejected                                       │
│  - Generation incremented again                                  │
└──────────────────────────────────────────────────────────────────┘
            ↓
            ├─ Recovery probe SUCCEEDS
            │      ↓
            │  record_success_with_permit()
            │      ↓
            │  reset() → Metrics cleared
            │      ↓
            └─→ CLOSED ✅
            
            ├─ Recovery probe FAILS
            │      ↓
            │  record_failure_with_permit()
            │      ↓
            │  trip() → Increment generation
            │      ↓
            └─→ OPEN (restart cooldown) 🔴
```

### Request Lifecycle With Generation Tracking

```
Time  State  Generation  Request                  Metrics?
────  ─────  ──────────  ────────────────────────────────────
0     CLOSED 0           Normal request acquired  ✅ Tracked
1              ├─ Succeeds → record_success()
               └─ Permit.gen=0 matches current.gen=0 → Counted

      │
      ├─ Error threshold exceeded
      └─ trip() called

2     OPEN   1           New request blocked      ✗ Rejected

3              ├─ Cooldown (60s) passes
               └─ tick() → state = HALF_OPEN

4     HALF   1           Recovery probe acquired  ✅ Probe slot
       OPEN              permit = RecoveryProbe
                         (generation=1)

5              ├─ Recovery probe fails
               ├─ record_failure_with_permit(permit.gen=1)
               ├─ Current gen=1 ✓ matches
               ├─ trip() → gen becomes 2
               └─ State = OPEN

6     OPEN   2           Requests from gen 0?     ✗ Ignored
                         (permit.gen=0 ≠ current.gen=2)
                         Their metrics not counted!
```

---

## Session Storage Data Loss Prevention (DATA-05)

### Write Path: Session Completion to Archive

```
Session becomes terminal (Completed, Failed, Escalated, Abandoned)
│
├─ User calls: tiered.finish(&session_id, terminal_state)
│
├─ [Step 1] CAS Update to Terminal State
│  ├─ fetch current version from hot store
│  ├─ CAS update: if hot.version == expected_version
│  │   └─ Update to new state with version++
│  │  (Atomic: concurrent turn addition fails gracefully)
│  │
│  └─ Why CAS? Prevent concurrent turn from being lost:
│     Time 0: finish() gets session v1
│     Time 1: Concurrent turn arrives, hot store becomes v2
│     Time 2: finish() CAS v1→v2 fails → Keep v2
│
├─ [Step 2] flush_to_cold() with Retry Loop
│  │
│  ├─ Retry 1 (Delay: 200ms)
│  │  ├─ PostgreSQL.archive(session)
│  │  │  ├─ Success? → Proceed to Step 3 ✅
│  │  │  ├─ StorageTransient (connection timeout)? → Retry
│  │  │  └─ Storage (non-retryable)? → Return error
│  │
│  ├─ Retry 2 (Delay: 400ms)
│  │
│  ├─ Retry 3 (Delay: 800ms)
│  │
│  ├─ Retry 4 (Delay: 1600ms)
│  │
│  └─ Retry 5 (Delay: 2000ms)
│     └─ All failed? → Return error, session stays in Redis ⚠️
│
├─ [Step 3] Version-Safe Redis Deletion
│  │
│  ├─ delete_if_version(session_id, archived_version)
│  │  (Implemented with Redis Lua script - atomic)
│  │
│  ├─ If current hot store version == archived_version
│  │  └─ Delete it ✅
│  │
│  ├─ If hot store version changed (concurrent update!)
│  │  └─ Keep it in Redis 🔄
│  │  └─ Log: "version changed, keep for next retry"
│  │
│  └─ If session already deleted from hot store
│     └─ No-op, return success
│
└─ End: Session either archived OR safely retained in Redis
```

### Retry Error Classification

```
PostgreSQL Error
├─ Connection Errors (RETRYABLE)
│  ├─ IO error (connection lost, network timeout)
│  ├─ PoolTimedOut (too busy, retry later)
│  ├─ PoolClosed (pool state issue)
│  └─ Tls (TLS negotiation blip)
│
├─ Transaction Conflicts (RETRYABLE)
│  ├─ 40001: Serialization failure
│  ├─ 40P01: Deadlock detected
│  ├─ 55P03: Lock not available
│  │
│  └─ Why retry? Concurrent transactions resolved, retry likely succeeds
│
├─ Server Status (RETRYABLE)
│  ├─ 53300: Too many connections (server overloaded)
│  ├─ 57P01: Admin shutdown
│  ├─ 57P02: Crash shutdown
│  └─ 57P03: Cannot connect now
│
└─ Schema/Data Errors (NON-RETRYABLE) ✗
   ├─ No table found
   ├─ Column not found
   ├─ Invalid data format
   ├─ Foreign key constraint violation
   │
   └─ Why not retry? Same code run again will fail identically
      (Need code fix, not retry)
```

### Recovery Paths

#### Path A: Retry on Later Request

```
Request 1: tiered.finish(&id, Completed)
├─ Archive attempt 1-5: All fail (PostgreSQL down)
└─ Session stays in Redis ⚠️

[PostgreSQL comes back online]

Request 2: User makes new request, calls tiered.finish(&id, Completed)
├─ Archive attempt 1: SUCCESS ✅
├─ Version-safe delete: Session removed from Redis
└─ Archive complete!
```

#### Path B: Background Sweeper Recovery

```
Every 30 seconds:
└─ sweeper.sweep_once()
   │
   ├─ Scan all sessions in Redis hot store
   │
   ├─ For each session:
   │  ├─ If state = Completed/Failed/Abandoned (terminal)
   │  │  └─ tiered.recover_terminal(&id)
   │  │     ├─ Retry archival (from hot only, don't rehydrate)
   │  │     └─ Success? → Delete and log
   │  │
   │  ├─ Else if inactive for 5+ minutes
   │  │  └─ tiered.abandon_if_inactive(&id, cutoff)
   │  │     ├─ Mark as Abandoned
   │  │     └─ Archive it
   │  │
   │  └─ Else skip (active session, let it live)
   │
   └─ Log: "Archived N sessions by sweeper/recovery"

[API Restarts]
├─ Sweeper starts again
├─ First sweep_once() catches any terminal sessions
│  (e.g., those that crashed during archival)
└─ Retries them all
```

#### Path C: Extended PostgreSQL Outage (TTL Protection)

```
PostgreSQL outage for 48 hours

Session Terminal State (Completed) in Redis:
├─ TTL = 72 hours (extended for terminal sessions)
├─ Sweeper tries every 30 seconds: Still failing
├─ After 24 hours: TTL still active (not expired)
├─ After 48 hours: TTL still active, PostgreSQL recovers
└─ Sweeper's next attempt: SUCCESS ✅

Without TTL protection:
├─ TTL = 24 hours (normal)
├─ After 25 hours: Session expires from Redis
├─ Data lost even though PostgreSQL will recover! ❌
```

---

## Concurrent Update Scenario (Version Safety)

```
Timeline: flush_to_cold() vs. Concurrent Webhook/Turn

Time  Event
────  ────────────────────────────────────────────────────────────
0ms   User calls tiered.finish(&session_id, Completed)
      └─ Fetches session from hot store: v=1

1ms   finish() performs CAS update to Completed
      └─ hot.update_cas(&session, v1→v2): SUCCESS
      └─ Session now in Redis with v=2

2ms   finish() calls flush_to_cold(&session)
      ├─ session object has v=1 (stale copy)
      └─ Begins archival of v=1

3ms   Concurrent webhook arrives with new turn
      ├─ session = hot.get(&session_id)
      │ └─ Gets v=2
      ├─ turn.record(new_turn)
      ├─ session.version++ (now v=3)
      └─ hot.update_cas(&session, v2→v3): SUCCESS

4ms   Archive complete: PostgreSQL has session v=1

5ms   delete_if_version(&session_id, expected_v=1)
      ├─ hot.get(&session_id).version = 3
      ├─ 3 ≠ 1 → Don't delete! ✅
      └─ Log: "version changed, keep for next retry"

6ms   Session remains in Redis with v=3
      └─ New turn from step 3ms is preserved! ✅

Next sweep/request:
├─ tiered.recover_terminal(&session_id)
├─ Fetches v=3 from hot store
├─ Archives it (overwriting v=1)
└─ Now safe to delete ✅
```

---

## Circuit Breaker + Session Store Integration

```
┌────────────────────────────────────────────────┐
│  Incoming LLM Request                          │
└────────────────────────────────────────────────┘
                     ↓
┌────────────────────────────────────────────────┐
│  [1] Circuit Breaker: breaker.acquire()?       │
│      ├─ Closed? → return RequestPermit::Normal │
│      ├─ Open? → return Err(CircuitOpen)        │
│      └─ Half-Open? → return Probe permit or error
└────────────────────────────────────────────────┘
                     ↓
                     ├─ Rejected (circuit open)
                     │  └─ Return error to caller
                     │
                     ├─ Permitted
                     │  ↓
         ┌───────────────────────────┐
         │  [2] Load Session         │
         │      from Tiered Store    │
         │      ├─ Hot (Redis)? ✅   │
         │      └─ Cold (PG)? 🔄     │
         └───────────────────────────┘
                     ↓
         ┌───────────────────────────┐
         │  [3] Call LLM API         │
         │      (measure latency)    │
         └───────────────────────────┘
                     ↓
                     ├─ LLM Success (200ms)
                     │  ├─ breaker.record_success_with_permit(permit, 200)
                     │  │  ├─ Metrics updated
                     │  │  ├─ If Half-Open & success → reset() to Closed
                     │  │  └─ Permit generation matches current
                     │  │
                     │  ├─ Save response to session
                     │  ├─ hot.put(&session) [fast]
                     │  └─ No archive yet (session still active)
                     │
                     └─ LLM Error (timeout, 5s)
                        ├─ breaker.record_failure_with_permit(permit, 5000, distress)
                        │  ├─ Metrics updated
                        │  ├─ If Half-Open → trip() back to Open
                        │  ├─ Check thresholds (sentiment-adjusted)
                        │  └─ If threshold exceeded → trip() to Open
                        │
                        └─ Return error to caller

[Later when session completes]
                     ↓
         ┌───────────────────────────────────┐
         │  [4] Session Terminal              │
         │      tiered.finish(&id, Completed)│
         │      ├─ [Retry Loop] Archive to PG│
         │      ├─ Exponential backoff        │
         │      └─ Version-safe deletion      │
         │         from Redis                 │
         └───────────────────────────────────┘
```

---

## State Diagram: Session Lifecycle vs. Storage

```
┌─────────────────┐
│ CREATED (Hot)   │
│ In Redis only   │
└────────┬────────┘
         │
         ↓ (first turn)
┌─────────────────┐
│ ACTIVE (Hot)    │
│ In Redis        │
│ Can receive     │
│ new turns       │
└────────┬────────┘
         │ (business logic end)
         ↓
┌──────────────────────────────────────┐
│ TERMINAL (Completed/Failed/Escalated)│
│                                      │
│ Hot: Still in Redis                  │
│ ├─ flush_to_cold() with retry        │
│ ├─ Archive success → delete          │
│ └─ Archive fail → keep in Redis      │
│                                      │
│ Cold: PostgreSQL (if archive ok)     │
│ ├─ Full history preserved            │
│ └─ Reachable for analytics           │
└──────────────────────────────────────┘
         │
         ├─ Archive Success
         │  ├─ Delete from Redis
         │  └─ Cold only
         │
         └─ Archive Failed
            ├─ Keep in Redis
            ├─ Extended TTL (72h)
            └─ Sweeper retries

    [PostgreSQL recovers]
         │
         ├─ Sweeper detects terminal
         │  in Redis
         │  └─ recover_terminal()
         │
         ├─ Archive succeeds on retry
         │  └─ Version-safe delete
         │
         └─ Now cold-only
```

---

## Test Coverage Map

### Circuit Breaker (16 tests)

```
acquire()
├─ Normal Closed → RequestPermit::Normal ✅
└─ Half-Open → RequestPermit::RecoveryProbe or Error ✅

State Transitions
├─ Closed → Open (error/latency threshold) ✅
├─ Open → Half-Open (cooldown elapsed) ✅
├─ Half-Open → Closed (recovery success) ✅
└─ Half-Open → Open (recovery failure) ✅

Metrics & P95
├─ Sliding window capacity ✅
├─ Empty window handling ✅
├─ P95 calculation (not average) ✅
└─ Error rate with window slide ✅

Generation Isolation (prevents stale interference)
├─ Generation packed/unpacked correctly ✅
├─ Old recovery probe can't close new gen ✅
├─ Old normal request can't affect half-open ✅
├─ Old failure can't trip new gen ✅
└─ Old failure can't reopen half-open ✅

Concurrency
├─ Only one recovery probe in Half-Open ✅
└─ Thread-safe state transitions ✅
```

### Session Store (9 tests)

```
Retry Mechanism
├─ 2 failures → 3rd succeeds ✅
├─ 5 failures → keep session in Redis ✅
└─ Later request retries again ✅

Version Safety
├─ Newer version protected (stale archive) ✅
├─ Concurrent update preserved ✅
└─ CAS conflict prevents archive ✅

Recovery Paths
├─ Background sweeper recovers terminal ✅
├─ Stale archive doesn't delete newer ✅
└─ Archive before delete order ✅

State Preservation
├─ Terminal state persisted ✅
└─ Cold store can rehydrate ✅
```
