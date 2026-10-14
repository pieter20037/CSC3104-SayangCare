# Merged Implementations Review Guide

This guide covers two merged pull requests:

- **CB-01 & CB-02**: Circuit Breaker Implementation (merged to main by Yi Xuan)
- **DATA-05**: PostgreSQL Retry Mechanism & Session Data Loss Prevention (merged to main by Lucas)

## Overview

### CB-01 & CB-02: Circuit Breaker Pattern

**Location**: [`crates/circuit-breaker/src/`](../crates/circuit-breaker/src/)

The circuit breaker protects the LLM API from cascading failures when the backend is degraded or unavailable. It implements the classic state machine:

```
Closed → Open → Half-Open → Closed
         ↑_________________↓
```

**Key Features:**

- **State Transitions**: Automated transitions based on error rate and P95 latency thresholds
- **Adaptive Thresholds**: Thresholds scale based on caller distress severity (sentiment-aware)
- **Generation Tracking**: Atomic state + generation prevents stale requests from interfering with recovery
- **Recovery Probes**: Only one request permitted during Half-Open to test if the service recovered
- **Request Permits**: Distinguishes normal requests from recovery probes

---

## CB-01 & CB-02: Circuit Breaker Implementation

### Files Modified

1. **`crates/circuit-breaker/src/breaker.rs`** - Main implementation
2. **`crates/api/src/handlers/call.rs`** - Integration with LLM call handler
3. **`crates/api/src/handlers/twilio.rs`** - Integration with Twilio webhook handler

### How It Works

#### State Machine

```
┌─────────────────────────────────────────────────────────────────┐
│                                                                   │
│  CLOSED (Normal operation)                                       │
│  ├─ Requests pass through normally                               │
│  ├─ Metrics collected: error rate, P95 latency                  │
│  └─ If threshold exceeded → trip() → OPEN                       │
│                                                                   │
│  ↓                                                                │
│                                                                   │
│  OPEN (Circuit breaker tripped)                                  │
│  ├─ All requests rejected with `CircuitOpen` error              │
│  ├─ Cooldown timer starts (configurable duration)              │
│  ├─ Generation incremented (invalidates old requests)           │
│  └─ After cooldown expires → tick() → HALF_OPEN                │
│                                                                   │
│  ↓                                                                │
│                                                                   │
│  HALF_OPEN (Recovery mode)                                       │
│  ├─ Only ONE request permitted (recovery probe)                │
│  ├─ Other requests rejected                                     │
│  ├─ If probe succeeds → reset() → CLOSED                       │
│  ├─ If probe fails → trip() → OPEN                             │
│  └─ Generation incremented again                                │
│                                                                   │
└─────────────────────────────────────────────────────────────────┘
```

#### Key Concepts

**RequestPermit Enum**:

```rust
pub enum RequestPermit {
    Normal { generation: u64 },         // Normal request in Closed state
    RecoveryProbe { generation: u64 },  // Sole permitted request in Half-Open
}
```

**Generation Tracking**:

- Each time the circuit opens, generation increments
- Normal requests only count toward metrics if their generation matches current generation
- Prevents old requests from affecting new recovery attempts

**Packed State**:

```rust
// 64-bit value: [generation (62 bits) | state (2 bits)]
fn pack_state(generation: u64, state: u8) -> u64 {
    (generation << STATE_BITS) | u64::from(state)
}
```

### Testing the Circuit Breaker

#### Run Unit Tests

```powershell
cargo test -p sayangcare-circuit-breaker -- --nocapture
```

**Expected Output** (16 tests should pass):

```
running 16 tests
test breaker::tests::acquire_returns_correct_request_permit ... ok
test breaker::tests::empty_window_returns_zero_metrics ... ok
test breaker::tests::error_rate_updates_when_window_slides ... ok
test breaker::tests::failed_recovery_probe_reopens_circuit ... ok
test breaker::tests::failed_recovery_allows_another_probe_after_cooldown ... ok
test breaker::tests::half_open_allows_only_one_recovery_probe ... ok
test breaker::tests::old_normal_failure_must_not_reopen_half_open_circuit ... ok
test breaker::tests::old_normal_request_must_not_close_half_open_circuit ... ok
test breaker::tests::outdated_failure_cannot_trip_new_generation ... ok
test breaker::tests::outdated_recovery_probe_cannot_close_new_generation ... ok
test breaker::tests::p95_is_not_average_latency ... ok
test breaker::tests::packed_state_preserves_generation_and_state ... ok
test breaker::tests::sliding_window_discards_old_requests ... ok
test breaker::tests::successful_recovery_probe_closes_circuit ... ok
test breaker::tests::valid_recovery_probe_closes_circuit_with_permit ... ok
test breaker::tests::concurrent_half_open_requests_allow_only_one_probe ... ok

test result: ok. 16 passed; 0 failed; 0 ignored
```

### Test Breakdown

| Test Name                                              | What It Tests                    | Why It Matters                                         |
| ------------------------------------------------------ | -------------------------------- | ------------------------------------------------------ |
| `sliding_window_discards_old_requests`                 | Metrics window respects capacity | Ensures old metrics don't skew current health          |
| `empty_window_returns_zero_metrics`                    | Edge case: no requests yet       | Prevents division by zero / panic                      |
| `p95_is_not_average_latency`                           | P95 calculation correctness      | Tail latency matters more for circuit breaking         |
| `error_rate_updates_when_window_slides`                | Error rate recalculation         | Sliding window must update error statistics            |
| `half_open_allows_only_one_recovery_probe`             | Concurrency control in Half-Open | Prevents race conditions during recovery               |
| `successful_recovery_probe_closes_circuit`             | Recovery path (success)          | Circuit can heal after outage                          |
| `failed_recovery_probe_reopens_circuit`                | Recovery path (failure)          | Circuit remains protected if service still broken      |
| `failed_recovery_allows_another_probe_after_cooldown`  | Retry mechanism                  | Service gets multiple chances to recover               |
| `acquire_returns_correct_request_permit`               | Permit logic                     | Callers know if they have recovery slot or normal slot |
| `concurrent_half_open_requests_allow_only_one_probe`   | Concurrency safety               | Only one thread gets recovery probe                    |
| `outdated_recovery_probe_cannot_close_new_generation`  | Generation isolation             | Old recovery attempt can't close new generation        |
| `valid_recovery_probe_closes_circuit_with_permit`      | Permit-based state change        | Recovery probe correctly closes circuit                |
| `old_normal_request_must_not_close_half_open_circuit`  | Generation isolation             | Stale requests don't interfere with recovery           |
| `old_normal_failure_must_not_reopen_half_open_circuit` | Generation isolation             | Stale failures don't interfere with recovery           |
| `outdated_failure_cannot_trip_new_generation`          | Generation isolation             | Stale failures don't trip new generation               |
| `packed_state_preserves_generation_and_state`          | Atomic packing/unpacking         | State representation is correct                        |

### Integration Points

#### 1. LLM Call Handler (`crates/api/src/handlers/call.rs`)

```rust
let permit = breaker.acquire()?;  // Get permit or fail if Open

// Call LLM
let llm_response = llm_service.call(&request).await;

match llm_response {
    Ok(response) => {
        let latency = elapsed.as_millis() as u64;
        breaker.record_success_with_permit(permit, latency);
        Ok(response)
    }
    Err(error) => {
        let latency = elapsed.as_millis() as u64;
        breaker.record_failure_with_permit(permit, latency, distress_severity);
        Err(error)
    }
}
```

#### 2. Twilio Webhook Handler (`crates/api/src/handlers/twilio.rs`)

Similar integration pattern for webhook processing.

---

## DATA-05: PostgreSQL Retry Mechanism & Data Loss Prevention

**Location**: [`crates/session-store/src/`](../crates/session-store/src/)

This feature ensures that sessions never get lost when PostgreSQL archival fails. It combines retry logic with atomic deletion and background recovery.

### Files Modified

1. **`crates/session-store/src/tiered.rs`** - Retry logic, version-safe deletion, recovery
2. **`crates/session-store/src/cold.rs`** - PostgreSQL archive store with error classification
3. **`crates/session-store/src/sweeper.rs`** - Background recovery task

### How It Works

#### Architecture: Tiered Storage

```
┌──────────────────────────────────────────────────────────┐
│  Redis (Hot Store)                                        │
│  ├─ Fast access (~1ms)                                   │
│  ├─ Session data + latest turns                          │
│  └─ TTL protection for terminal sessions awaiting archive│
├──────────────────────────────────────────────────────────┤
│  PostgreSQL (Cold Store)                                  │
│  ├─ Durable archival                                      │
│  ├─ Terminal sessions (Completed, Failed, Escalated)     │
│  └─ Full transcript history                              │
└──────────────────────────────────────────────────────────┘
```

#### Write Path

```
Session Completes
  ↓
finish(&session) called
  ├─ CAS update to terminal state
  │  (Atomic: prevents concurrent turn loss)
  ↓
flush_to_cold()
  ├─ Retry Loop (5 attempts, exponential backoff)
  │  ├─ Attempt 1: 200ms delay
  │  ├─ Attempt 2: 400ms delay
  │  ├─ Attempt 3: 800ms delay
  │  ├─ Attempt 4: 1600ms delay
  │  └─ Attempt 5: 2000ms delay (capped)
  │
  ├─ PostgreSQL Outage? → Retryable error
  ├─ Concurrent update? → Stay in Redis
  ├─ Archive success? → Delete Redis version-safely
  └─ Archive failed after 5 attempts → Keep in Redis

Session awaiting archive stays in Redis
  ├─ If update happens: version++ (preserved)
  └─ If no update: TTL protection keeps it until sweeper retries
```

#### Retry Error Classification

```rust
// PostgreSQL errors that should retry:
// - "40001": Serialization failure (transaction conflict)
// - "40P01": Deadlock detected
// - "55P03": Lock not available
// - "53300": Too many connections
// - "57P01": Admin shutdown
// - "57P02": Crash shutdown
// - "57P03": Cannot connect now
// - IO errors, connection timeouts, TLS errors

// Non-retryable errors:
// - Table not found
// - Schema validation failure
// - Invalid data format
```

#### Version-Safe Deletion

```rust
// Problem: Redis session updates to v2 while archiving v1
//
// Solution: CAS delete only the exact version that was archived
//
// delete_if_version(session_id, expected_version: v1)
//   ├─ If current version == v1
//   │  └─ Delete (clean up successful)
//   └─ If current version != v1 (concurrent update happened)
//      └─ Don't delete (v2 is newer and must survive)
//
// Implemented with Redis Lua scripting (atomic at Redis level)
```

#### Background Sweeper Recovery

```
Every 30 seconds (sweep_once):

For each session in Redis:
  ├─ Terminal (Completed/Failed/Abandoned)?
  │  └─ recover_terminal() → Retry archival
  │
  └─ Inactive for 5+ minutes?
     └─ abandon_if_inactive() → Mark Abandoned + Archive

Recovery also runs:
  ├─ On API startup (catches crashed archival attempts)
  └─ Periodically (catches PostgreSQL outages that resolved)
```

### Testing DATA-05

#### Run Unit Tests

```powershell
cargo test -p sayangcare-session-store -- --nocapture
```

**Expected Output** (9 tests should pass):

```
running 9 tests
test tiered::tests::archive_recovers_on_later_request ... ok
test tiered::tests::background_recovery_archives_completed_session_after_failure ... ok
test tiered::tests::cold_read_rehydrates_full_session_into_hot_store ... ok
test tiered::tests::finish_cas_conflict_does_not_archive_or_delete_session ... ok
test tiered::tests::finish_commits_archive_before_removing_hot_session ... ok
test tiered::tests::finish_preserves_escalated_state_for_high_risk_calls ... ok
test tiered::tests::retry_exhaustion_preserves_hot_session ... ok
test tiered::tests::retry_succeeds_after_temporary_failures ... ok
test tiered::tests::stale_archive_does_not_delete_newer_hot_version ... ok

test result: ok. 9 passed; 0 failed; 0 ignored
```

### Test Breakdown

| Test Name                                                      | What It Tests         | Why It Matters                                                 |
| -------------------------------------------------------------- | --------------------- | -------------------------------------------------------------- |
| `retry_succeeds_after_temporary_failures`                      | Retry mechanism       | Session survives 2 failures, succeeds on 3rd attempt           |
| `retry_exhaustion_preserves_hot_session`                       | Data loss prevention  | After 5 failed attempts, session stays in Redis (not deleted!) |
| `background_recovery_archives_completed_session_after_failure` | Sweeper recovery      | Failed terminal archival retried by background task            |
| `archive_recovers_on_later_request`                            | Deferred recovery     | If first finish() fails, retry on second finish() succeeds     |
| `stale_archive_does_not_delete_newer_hot_version`              | Version-safe deletion | Older version doesn't delete newer concurrent update           |
| `finish_commits_archive_before_removing_hot_session`           | CAS semantics         | Archive succeeds before Redis cleanup                          |
| `finish_cas_conflict_does_not_archive_or_delete_session`       | Concurrency safety    | Concurrent update prevents stale archival                      |
| `finish_preserves_escalated_state_for_high_risk_calls`         | State preservation    | Terminal state is correctly persisted                          |
| `cold_read_rehydrates_full_session_into_hot_store`             | Fallback mechanism    | Can reconstruct session from PostgreSQL if Redis lost          |

### Key Data Loss Prevention Techniques

#### 1. Atomic Archive-Then-Delete

```rust
// ✓ Correct: Archive first, then delete
for attempt in 1..=MAX_ATTEMPTS {
    match self.cold.archive(session).await {
        Ok(()) => {
            // Archive succeeded; now safe to delete
            let deleted = self.hot.delete_if_version(
                &session.id,
                session.version
            ).await?;
            return Ok(());
        }
        Err(error) => {
            if retryable {
                sleep(exponential_backoff).await;  // Retry
            } else {
                return Err(error);  // Permanent error
            }
        }
    }
}

// ✗ Wrong: Delete first (data loss if archive fails!)
// self.hot.delete(&session.id).await?;
// self.cold.archive(session).await?;  // Too late if this fails
```

#### 2. Version Conflict Handling

```rust
// ✓ Concurrent turn arrives while archiving:
// Time 0: finish(session_id, Completed) starts
// Time 1: Session is in Redis v1, archived as v1
// Time 2: Concurrent turn arrives, updates to v2
// Time 3: delete_if_version(session_id, v1) returns false
//         (version changed, keep v2 in Redis)
// Time 4: Sweeper eventually archives v2

// ✗ If we used unconditional delete:
// Time 3: delete(session_id) → v2 is lost! (data loss)
```

#### 3. Terminal Session TTL Protection

```rust
// Sessions waiting for archival have extended TTL:
// - Normal active sessions: 24 hours
// - Terminal sessions awaiting archive: 72 hours
//
// Why? PostgreSQL outage might last 48 hours
// Without TTL protection: session expires from Redis
// With TTL protection: sweeper can still retry archival
```

### Integration with Sweeper

**File**: [`crates/session-store/src/sweeper.rs`](../crates/session-store/src/sweeper.rs)

```rust
pub async fn sweep_once(&self) -> Result<usize, CoreError> {
    const INACTIVITY: ChronoDuration = ChronoDuration::minutes(5);
    let cutoff = Utc::now() - INACTIVITY;

    for session in self.hot.scan_sessions().await? {
        let result = match session.state {
            // Recovery path: retry archival of terminal sessions
            SessionState::Completed | SessionState::Failed | SessionState::Abandoned => {
                self.tiered.recover_terminal(&session.id).await
            }
            // Cleanup path: mark inactive sessions as abandoned
            _ if session.updated_at < cutoff => {
                self.tiered.abandon_if_inactive(&session.id, cutoff).await
            }
            _ => continue,
        };

        match result {
            Ok(true) => archived += 1,
            Ok(false) => {},
            Err(error) => {
                // Keep session for retry (don't panic, don't delete)
                warn!("session recovery failed; retained for retry");
            }
        }
    }
}
```

---

## Running Complete Test Suite

### Prerequisites

Ensure Docker Desktop is running:

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

**Check for**:

- `postgres` status: `healthy`
- Ports available: `5434` (PostgreSQL), `6379` (Redis), `26379` (Sentinel)

### Run All Tests

```powershell
cd c:\Users\darre\CSC3104-SayangCare Project\CSC3104-SayangCare
cargo test --workspace
```

**Expected Summary Output**:

```
test result: ok. 1 passed ...          (sayangcare-api tests)
test result: ok. 16 passed ...         (circuit-breaker: 16 tests)
test result: ok. 13 passed ...         (core domain tests)
test result: ok. 8 passed; 1 ignored   (priority-queue: 8 tests, 1 Redis-dependent)
test result: ok. 9 passed ...          (session-store: 9 tests)
test result: ok. 2 passed ...          (telephony tests)
```

**Total: 48 tests passed, 1 ignored (normal)**

### Run Specific Test Suites

```powershell
# Circuit breaker tests only
cargo test -p sayangcare-circuit-breaker

# Session store tests only
cargo test -p sayangcare-session-store

# Core domain tests
cargo test -p sayangcare-core

# All tests with verbose output
cargo test --workspace -- --nocapture
```

---

## Key Architectural Decisions

### Circuit Breaker

| Decision                             | Rationale                                                      |
| ------------------------------------ | -------------------------------------------------------------- |
| **Atomic generation + state in u64** | Prevents ABA problem; stale requests can't interfere           |
| **Sentiment-aware thresholds**       | Distressed callers get stricter thresholds (faster protection) |
| **P95 latency metric**               | Tail latency matters more than average for UX                  |
| **Sliding window metrics**           | Simple, O(1) space, fast evaluation                            |
| **Only one recovery probe**          | Prevents thundering herd; ensures controlled recovery testing  |

### Session Storage

| Decision                            | Rationale                                              |
| ----------------------------------- | ------------------------------------------------------ |
| **Retry with exponential backoff**  | Allows temporary PostgreSQL blips to resolve naturally |
| **Archive-then-delete pattern**     | Never delete before confirming durability              |
| **Version-safe CAS deletion**       | Prevents concurrent updates from being lost            |
| **Terminal session TTL protection** | Survives extended outages (72 hours)                   |
| **Background sweeper recovery**     | Catches failures that occur between requests           |
| **Retryable error classification**  | Distinguishes transient vs. permanent failures         |

---

## Monitoring & Observability

### Circuit Breaker Metrics

The circuit breaker records these values:

```rust
pub struct BreakerSnapshot {
    pub state: CircuitState,           // Closed, Open, or Half-Open
    pub error_rate: f64,               // Percentage (0.0-1.0)
    pub p95_latency_ms: u64,           // Milliseconds
    pub total_requests: u64,           // In current window
    pub opened_at: Option<DateTime<Utc>>, // When last tripped
    pub last_adapted_threshold: f64,   // Sentiment-adjusted threshold
}
```

Access with:

```rust
let snapshot = breaker.snapshot();
println!("Circuit state: {:?}", snapshot.state);
println!("Error rate: {:.2}%", snapshot.error_rate * 100.0);
println!("P95 latency: {}ms", snapshot.p95_latency_ms);
```

### Session Store Metrics

Watch for these log messages:

```
INFO: PostgreSQL archive successful (attempt=1, version=42)
WARN: PostgreSQL archive attempt failed (attempt=3, retryable=true)
INFO: Redis session cleanup successful (version=42)
WARN: Redis cleanup skipped: session missing or version changed
INFO: sessions archived by sweeper (archived=15)
```

---

## Troubleshooting

### Circuit Breaker Stuck Open?

1. Check the cooldown duration (default 60 seconds)
2. Verify error threshold is reasonable
3. Check LLM service status
4. Monitor P95 latency - if consistently high, that's why it's open

**Solution**: Wait for cooldown, or temporarily increase error threshold

### Sessions Stuck in Redis?

1. Check PostgreSQL connectivity:
   ```powershell
   docker compose ps postgres
   ```
2. Check Redis TTL protection:
   ```powershell
   redis-cli TTL session:<id>
   ```
3. Check sweeper logs for recovery errors
4. Manually trigger recovery:
   ```powershell
   # Call the sweeper debug endpoint (if exposed)
   ```

**Solution**: Fix PostgreSQL connectivity, sweeper will auto-retry

### Version Conflict Errors?

This is **expected and correct**. It means:

- A concurrent turn arrived while archiving was in progress
- The system correctly preserved the newer update
- Next recovery attempt will archive the newer version

---

## Summary

| Component                | What It Does                                | Status                            |
| ------------------------ | ------------------------------------------- | --------------------------------- |
| **Circuit Breaker**      | Protects LLM API from cascading failures    | ✅ Merged (16 tests)              |
| **Retry Mechanism**      | Survives temporary PostgreSQL outages       | ✅ Merged (9 tests)               |
| **Data Loss Prevention** | Never deletes sessions prematurely          | ✅ Merged (version-safe deletion) |
| **Background Recovery**  | Recovers from crashes and extended outages  | ✅ Merged (sweeper task)          |
| **TTL Protection**       | Terminal sessions survive prolonged outages | ✅ Merged (72-hour TTL)           |

All implementations are tested, merged to main, and ready for production use.
