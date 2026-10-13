# Quick Reference: Running Tests & Expected Outputs

## Setup (One-time)

```powershell
# Start Docker dependencies
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel

# Verify they're healthy
docker compose ps
```

---

## Running Tests

### Option 1: All Tests (Recommended)

```powershell
cd "c:\Users\darre\CSC3104-SayangCare Project\CSC3104-SayangCare"
cargo test --workspace
```

**Expected Output Summary**:
- ✅ API tests: 1 passed
- ✅ Circuit breaker tests: 16 passed
- ✅ Core domain tests: 13 passed  
- ✅ Priority queue tests: 8 passed; 1 ignored (Redis-dependent, normal)
- ✅ Session store tests: 9 passed
- ✅ Telephony tests: 2 passed

**Total: 48 tests passed, 1 ignored**

---

### Option 2: Circuit Breaker Only (CB-01, CB-02)

```powershell
cargo test -p sayangcare-circuit-breaker
```

**Expected: 16 tests passed**

```
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

---

### Option 3: Session Store Only (DATA-05)

```powershell
cargo test -p sayangcare-session-store
```

**Expected: 9 tests passed**

```
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

---

### Option 4: Verbose Output (For Debugging)

```powershell
cargo test --workspace -- --nocapture
```

Shows all println! output and panics.

---

## What Each Test Verifies

### Circuit Breaker Tests (16)

**State Transitions** (3 tests):
- ✅ `half_open_allows_only_one_recovery_probe` → Only 1 request allowed in Half-Open
- ✅ `successful_recovery_probe_closes_circuit` → Half-Open + success → Closed
- ✅ `failed_recovery_probe_reopens_circuit` → Half-Open + failure → Open

**Metrics & Thresholds** (3 tests):
- ✅ `sliding_window_discards_old_requests` → Metrics window respects capacity
- ✅ `error_rate_updates_when_window_slides` → Error rate recalculated correctly
- ✅ `p95_is_not_average_latency` → P95 calculation is correct

**Generation Isolation** (5 tests):
- ✅ `packed_state_preserves_generation_and_state` → State format correct
- ✅ `outdated_recovery_probe_cannot_close_new_generation` → Old recovery blocked
- ✅ `old_normal_request_must_not_close_half_open_circuit` → Old normal blocked
- ✅ `outdated_failure_cannot_trip_new_generation` → Old failure can't trip
- ✅ `old_normal_failure_must_not_reopen_half_open_circuit` → Old failure blocked

**Recovery & Permits** (4 tests):
- ✅ `acquire_returns_correct_request_permit` → Permit type matches state
- ✅ `valid_recovery_probe_closes_circuit_with_permit` → Permit-based close works
- ✅ `failed_recovery_allows_another_probe_after_cooldown` → Retry after cooldown
- ✅ `concurrent_half_open_requests_allow_only_one_probe` → Concurrency safe
- ✅ `empty_window_returns_zero_metrics` → No panic on empty window

---

### Session Store Tests (9)

**Data Loss Prevention** (2 tests):
- ✅ `retry_exhaustion_preserves_hot_session` → Session stays in Redis after 5 failures
- ✅ `stale_archive_does_not_delete_newer_hot_version` → Newer versions protected

**Retry Mechanism** (3 tests):
- ✅ `retry_succeeds_after_temporary_failures` → Retries 2 failures, succeeds on 3rd
- ✅ `archive_recovers_on_later_request` → Deferred retry on next request
- ✅ `background_recovery_archives_completed_session_after_failure` → Sweeper recovery

**Concurrent Safety** (2 tests):
- ✅ `finish_cas_conflict_does_not_archive_or_delete_session` → Concurrent update safe
- ✅ `finish_commits_archive_before_removing_hot_session` → Archive-then-delete order

**State Preservation** (2 tests):
- ✅ `finish_preserves_escalated_state_for_high_risk_calls` → Terminal state saved
- ✅ `cold_read_rehydrates_full_session_into_hot_store` → Fallback mechanism works

---

## Failure Scenarios to Expect

### "Thread 'main' panicked"
→ A test assertion failed (data integrity issue)
→ Check the assertion message above the panic

### "StorageTransient" errors in logs
→ Simulated PostgreSQL outage (intentional in tests)
→ Retry mechanism should handle these

### "VersionConflict" errors
→ Concurrent update detected (correct behavior)
→ System should preserve newer version

### 1 ignored test in priority-queue
→ Requires Redis at 127.0.0.1:6379
→ Normal and expected; Docker provides this

---

## Verification Checklist

After running `cargo test --workspace`:

- [ ] **No red FAILED lines** (all tests green)
- [ ] **"test result: ok"** appears for each crate
- [ ] **48 total tests passed** (check final summary)
- [ ] **1 Redis test ignored** (normal)
- [ ] **No panics** (except intentional asserts in tests)
- [ ] **Build time ~1-2 minutes** (Rust compilation)

---

## Understanding Test Output Format

```
running 16 tests
test breaker::tests::half_open_allows_only_one_recovery_probe ... ok
         │                                                          │
         └─ Test name (file path + function)                       └─ Status (ok / FAILED / ignored)

test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured
             │  │           │          │           │
             │  └─ Passed    └─ Failed └─ Ignored  └─ Benchmarks (not in unit tests)
             └─ Overall result (ok / FAILED)
```

---

## Common Issues & Fixes

| Issue | Cause | Fix |
|-------|-------|-----|
| `error: could not find crate` | Missing dependencies | Run `cargo fetch` |
| `docker: command not found` | Docker not installed | Install Docker Desktop |
| `connection refused` | Docker containers not running | `docker compose up -d` |
| `FAILED [1 passed, 1 failed]` | Test assertion failed | Check test code/implementation |
| `1 ignored` in priority-queue | Redis test skipped | Expected; Docker provides Redis |

---

## Next Steps After Tests Pass

1. **Review implementations**:
   - See full guide: [`MERGED_IMPLEMENTATIONS_GUIDE.md`](./MERGED_IMPLEMENTATIONS_GUIDE.md)

2. **Check code**:
   - Circuit breaker: [`crates/circuit-breaker/src/breaker.rs`](./crates/circuit-breaker/src/breaker.rs)
   - Session store: [`crates/session-store/src/tiered.rs`](./crates/session-store/src/tiered.rs)

3. **Run the app** (if you want to see it in action):
   - Follow: [`PHASE_2.md`](./PHASE_2.md) for running Raft cluster

4. **Deploy to production**:
   - All tests passing → Code is production-ready
   - Merge to main (already done by Yi Xuan & Lucas)
