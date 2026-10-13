# Side-by-Side Comparison: Circuit Breaker vs. Session Store

## Problem → Solution

| Component | Problem | Solution | Files |
|-----------|---------|----------|-------|
| **Circuit Breaker (CB-01, CB-02)** | LLM API cascades failures under load/degradation | Automatic request rejection + recovery probe | `crates/circuit-breaker/src/breaker.rs` |
| **Session Store (DATA-05)** | PostgreSQL outage = data loss (deleted from Redis too soon) | Retry + version-safe deletion + background recovery | `crates/session-store/src/tiered.rs` |

---

## Key Mechanisms Comparison

### State Management

| Aspect | Circuit Breaker | Session Store |
|--------|-----------------|---------------|
| **States** | Closed, Open, Half-Open | Hot (Redis), Cold (PostgreSQL), Recovery |
| **Transitions** | Time-based (cooldown) + Event-based (success/failure) | Request-based + Time-based (sweeper) |
| **Atomicity** | Atomic u64 (generation + state) | Lua script (Redis) + CAS update |
| **Thread Safety** | `compare_exchange()` loop | Mutex + atomic CAS operations |

### Error Handling

| Aspect | Circuit Breaker | Session Store |
|--------|-----------------|---------------|
| **Error Types** | Latency, Error rate | Transient (connection), Permanent (schema) |
| **Retry Strategy** | No retry (fast fail) | Retry with exponential backoff |
| **Max Attempts** | 1 recovery probe | 5 attempts (200ms → 2s delay) |
| **Fallback** | Reject request | Keep session in Redis, retry later |

### Concurrency Safety

| Aspect | Circuit Breaker | Session Store |
|--------|-----------------|---------------|
| **Race Condition** | Stale requests from old generation | Concurrent updates during archival |
| **Protection** | Generation increment (invalidates old) | Version field (CAS check) |
| **Solution** | `permit.generation` must match current | `delete_if_version(expected_v)` only deletes match |
| **Test Case** | `outdated_failure_cannot_trip_new_generation` | `stale_archive_does_not_delete_newer_hot_version` |

---

## Test Coverage Breakdown

### Circuit Breaker: 16 Tests

**Category 1: Basic State Transitions (4 tests)**
```
Closed + threshold exceeded → Open
Open + cooldown elapsed → Half-Open
Half-Open + success → Closed
Half-Open + failure → Open
```

**Category 2: Metrics & Math (3 tests)**
```
Sliding window respects capacity
Error rate updates on window slide
P95 calculation (not average)
```

**Category 3: Generation Isolation (5 tests)**
```
Stale recovery probe can't close new generation
Stale normal request can't affect half-open
Stale failure can't trip new generation
Stale failure can't reopen half-open
Generation packing/unpacking correct
```

**Category 4: Concurrency & Recovery (4 tests)**
```
Only one recovery probe in Half-Open
Recovery probe closes circuit with permit
Failed recovery allows another probe after cooldown
Concurrent half-open requests (thread-safe)
```

### Session Store: 9 Tests

**Category 1: Retry Mechanism (3 tests)**
```
2 failures → 3rd succeeds
5 failures → keep in Redis (not deleted!)
Later request retries again
```

**Category 2: Version-Safe Deletion (2 tests)**
```
Newer version protected from stale archive
Concurrent update not deleted by stale version
```

**Category 3: Recovery Paths (2 tests)**
```
Background sweeper recovers terminal sessions
Failed archival retried on next request
```

**Category 4: State Preservation (2 tests)**
```
Terminal state persisted to PostgreSQL
Cold store can rehydrate active session
```

---

## Execution Flow Comparison

### Circuit Breaker Flow

```
User Request
    ↓
breaker.acquire()
    ├─ Closed? → return Normal permit ✅
    ├─ Open? → return Err(CircuitOpen) ❌
    └─ Half-Open? → return Probe permit or Err ⏳
    ↓ [Permitted]
Execute (call LLM)
    ↓
Success/Failure
    ↓
record_success_with_permit() OR record_failure_with_permit()
    ├─ Check generation matches current
    ├─ Update metrics
    └─ If Half-Open + success → reset() to Closed
```

### Session Store Flow

```
Session Completes
    ↓
tiered.finish(&session_id, terminal_state)
    ├─ [Step 1] CAS update to terminal
    │  └─ Atomic: prevents concurrent loss
    ├─ [Step 2] flush_to_cold() with retry
    │  └─ Retry Loop (5 attempts, exponential backoff)
    │     ├─ Success? → Step 3
    │     └─ Failure? → Keep in Redis
    └─ [Step 3] delete_if_version()
       ├─ Version matches? → Delete
       └─ Version changed? → Keep for next retry
    ↓
    └─ Session archived OR safely retained

[Background Sweeper: every 30 seconds]
    ├─ Scan Redis sessions
    ├─ Terminal sessions → recover_terminal()
    └─ Inactive sessions → abandon_if_inactive()
```

---

## Code Pattern Comparison

### Pattern 1: Atomic State Management

**Circuit Breaker**:
```rust
// Pack generation + state into single u64
fn pack_state(generation: u64, state: u8) -> u64 {
    (generation << STATE_BITS) | u64::from(state)
}

// Atomic compare-exchange loop
loop {
    let current = packed_state.load(Ordering::Acquire);
    let next = pack_state(new_generation, new_state);
    
    if packed_state.compare_exchange(
        current, next,
        Ordering::AcqRel,
        Ordering::Acquire,
    ).is_ok() {
        break;  // Success
    }
    // Retry on conflict
}
```

**Session Store**:
```rust
// CAS update with version check
async fn update_cas(&self, session: &Session, expected_version: u64) 
    -> CoreResult<()> 
{
    if current_version != expected_version {
        return Err(VersionConflict);
    }
    sessions.insert(session.id.clone(), session.clone());
    Ok(())
}

// Version-safe deletion (Lua script)
delete_if_version(&session_id, expected_version)
    // Only delete if version matches
```

### Pattern 2: Error Handling

**Circuit Breaker** (Fast Fail):
```rust
pub fn acquire(&self) -> Result<RequestPermit, CircuitError> {
    match state {
        Closed => Ok(permit),
        Open => Err(CircuitOpen),  // Reject immediately
        HalfOpen => {
            // Try to get probe slot
            if can_reserve_probe() {
                Ok(permit)
            } else {
                Err(CircuitOpen)  // Probe slot taken
            }
        }
    }
}
```

**Session Store** (Resilient):
```rust
for attempt in 1..=MAX_ATTEMPTS {
    match cold.archive(session).await {
        Ok(()) => {
            // Delete only if archive confirmed
            hot.delete_if_version(&session.id, version).await?;
            return Ok(());
        }
        Err(error) if error.is_retryable() => {
            // Retry with backoff
            sleep(exponential_backoff(attempt)).await;
        }
        Err(error) => {
            // Non-retryable: keep in Redis
            return Err(error);
        }
    }
}
```

### Pattern 3: Concurrency Safety

**Circuit Breaker** (Generation Isolation):
```rust
pub fn record_success_with_permit(
    &self, 
    permit: RequestPermit, 
    latency_ms: u64,
) {
    let current_generation = self.packed_state.load(Ordering::Acquire).generation;
    let request_generation = permit.generation;
    
    if request_generation != current_generation {
        return;  // Ignore stale request
    }
    
    // Only count metrics for current generation
    self.metrics.record(latency_ms, true);
}
```

**Session Store** (Version Checking):
```rust
pub async fn delete_if_version(
    &self,
    id: &SessionId,
    expected_version: u64,
) -> CoreResult<bool> {
    let current = sessions.get(id);
    
    if current.version == expected_version {
        sessions.remove(id);
        Ok(true)  // Deleted
    } else {
        Ok(false)  // Version changed, keep it
    }
}
```

---

## When Each Mechanism Triggers

### Circuit Breaker Recovery Scenarios

| Scenario | Action | Recovery |
|----------|--------|----------|
| LLM timeout (5s) | record_failure() | If threshold exceeded → trip() |
| LLM returns 500 | record_failure() | If threshold exceeded → trip() |
| Circuit open | All requests rejected | Wait for cooldown |
| Cooldown elapsed | State → Half-Open | Allow 1 recovery probe |
| Probe succeeds (200ms) | record_success() | reset() → Closed |
| Probe fails (5s) | record_failure() | trip() → Open again |

### Session Store Recovery Scenarios

| Scenario | Action | Recovery |
|----------|--------|----------|
| PostgreSQL down | Archive fails (transient) | Retry with backoff |
| All 5 retries fail | Archive gave up | Keep session in Redis |
| Session sits in Redis | Sweeper runs (30s) | recover_terminal() retry |
| PostgreSQL recovers | Sweeper detects terminal | Archive succeeds, delete |
| Concurrent update | Version changed | Keep newer version, retry later |
| API crashes mid-archive | Session stays in Redis | On startup, sweeper retries |

---

## Monitoring & Observability

### Circuit Breaker Observability

```rust
let snapshot = breaker.snapshot();

// Useful metrics:
println!("State: {:?}", snapshot.state);           // Closed/Open/Half-Open
println!("Error rate: {:.1}%", snapshot.error_rate * 100.0);  // 0-100%
println!("P95 latency: {}ms", snapshot.p95_latency_ms);       // In milliseconds
println!("Total requests: {}", snapshot.total_requests);       // In window
println!("Opened at: {:?}", snapshot.opened_at);              // When tripped

// Alert conditions:
if snapshot.state == CircuitState::Open && 
   (now - opened_at).seconds() > 300 {
    alert("Circuit breaker stuck open for 5+ minutes!");
}
```

### Session Store Observability

```
Logs:
INFO: PostgreSQL archive successful (attempt=1, version=42)
WARN: PostgreSQL archive failed (attempt=3, retryable=true)
INFO: Redis session cleanup successful (version=42)
INFO: sessions archived by sweeper (archived=15)

Metrics:
- Archive success rate (should be >99%)
- Average retry attempts (should be <1.1)
- Sessions in Redis (should trend to 0)
- Sweeper recovery frequency (should be low)

Alert conditions:
if archive_failure_rate > 0.05 {
    alert("PostgreSQL archival failing!");
}
if sessions_in_redis > 100 && age > 2_hours {
    alert("Sessions stuck in Redis (archive issue)!");
}
```

---

## Summary Table

| Feature | Circuit Breaker | Session Store |
|---------|-----------------|---------------|
| **Purpose** | Protect from cascading failures | Ensure data durability |
| **State Machine** | 3 states (Closed/Open/Half-Open) | 2 tiers (Hot/Cold) + Recovery |
| **Metrics** | Error rate, P95 latency | Version, attempt count |
| **Retry Strategy** | No retry (fast fail) | 5 retries with exponential backoff |
| **Concurrency** | Generation tracking | Version-safe CAS deletion |
| **Tests** | 16 unit tests | 9 unit tests |
| **Recovery** | Automatic via cooldown + probe | Automatic via sweeper |
| **Success Rate** | Should be 99%+ under normal load | Should be 99.9%+ |
| **Key Risk** | Circuit stuck open (manual recovery) | Sessions stuck in Redis (low TTL issue) |
| **Production Ready** | ✅ YES | ✅ YES |

