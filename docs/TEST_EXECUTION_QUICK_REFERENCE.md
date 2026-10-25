# Quick Reference: Running Tests

Commands in this guide work from PowerShell on Windows and Terminal on macOS unless a shell is named explicitly. Run commands from the repository root.

## Optional Redis Integration Tests

The normal workspace test run ignores two tests that need a live Redis service. Start Redis with Docker Compose if you want to include them:

```sh
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

## Run Tests

```sh
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
```

Workspace verification on 25 October 2026:

| Crate           | Passed | Ignored |
| --------------- | -----: | ------: |
| API             |     31 |       0 |
| Circuit breaker |     19 |       1 |
| Core            |     20 |       0 |
| Priority queue  |      8 |       3 |
| Redis provider  |      0 |       0 |
| Session store   |     10 |       0 |
| Telephony       |      2 |       0 |
| **Total**       | **90** | **4** |

Test totals can change as tests are added. The four ignored tests require live
Redis and were not counted as passed in this workspace run.

## Focused Tests

```sh
cargo test -p sayangcare-api
cargo test -p sayangcare-circuit-breaker
cargo test -p sayangcare-core
cargo test -p sayangcare-priority-queue
cargo test -p sayangcare-session-store
cargo test -p sayangcare-telephony
```

Run tests with captured output disabled when debugging:

```sh
cargo test --workspace -- --nocapture
```

To run a Redis-dependent test, start the Docker dependencies above, then run:

```sh
cargo test -p sayangcare-priority-queue --lib -- --include-ignored
cargo test -p sayangcare-circuit-breaker --lib -- --include-ignored
```

## Interpreting Results

- `test result: ok` means the tests in that crate passed.
- A nonzero `FAILED` count indicates a failing test; inspect the failure output before changing code.
- `connection refused` for Redis usually means the Docker services are stopped. Start them with the Compose command above.
- A `StorageTransient` error emitted inside a retry test can be expected test input; confirm the test itself ends in `ok`.
- A `VersionConflict` response means a stale concurrent update was rejected; it is an intentional consistency safeguard.

These are code-level checks. They do not prove live Twilio connectivity, Kubernetes autoscaling, Redis Sentinel failover, or API pod-recovery behavior. Use [Phase 4 Kubernetes guide](PHASE_4_KUBERNETES.md) for cluster verification.
