# Queue Claim and Load-Test Work (FIX-02, TEST-01, TEST-02, TEST-05)

This guide explains the queue regression, the test changes, and how to repeat
the load, pod-failure, and HPA checks. All runs use generated `SIM-` calls and
must target a test environment, never production. Run pod deletion only against
a disposable test cluster.

## Work summary

| ID | Change / evidence | Current status |
| --- | --- | --- |
| **FIX-02** | Prevented re-enqueueing an actively claimed Redis queue item; Redis claim Lua now protects against older stale duplicates; "claim next" discards entries whose session is missing or no longer pending. Added ignored Redis tests for stale/re-enqueued entries and concurrent claims. | Code and tests added; run the Redis integration tests below. |
| **TEST-01** | k6 now validates response bodies, reads each finished session back through the API, checks the persisted transcript, and records consistency/retry metrics as well as HTTP latency and errors. | Harness ready; rerun to create evidence for these additional checks. |
| **TEST-02** | The same lifecycle scenario supports active pod replacement. Read-only archive checks retry transient failures, and their retry count is measured; mutating incoming/turn/hangup calls are not retried. | Follow the controlled procedure below and save the k6 summary and recovery timestamps. |
| **TEST-05** | Added a Windows PowerShell runner that samples HPA current/desired/ready replicas during load and cooldown, records scale-up/down delay, and saves the k6 summary with p95/p99 and error metrics. It uses the existing HPA; it does not change its configuration. | Harness ready; run against the request-rate HPA overlay and record the output. |

The existing [local results](./PHASE_5_LOCAL_RESULTS.md) preserve earlier
100-VU baseline and pod-replacement numbers. Those summaries were produced
before the new body, archive, and consistency checks, so they are historical
HTTP-status evidence and must not be described as a pass of the stronger
checks in the current k6 script.

## FIX-02: Why Claim Next could return 409

The Redis queue stores pending sessions in a sorted set and active claim owners
in a hash. Repeated high-risk turns can enqueue an already-escalated session
again. If a volunteer had already claimed it, the old Redis enqueue path put
that session back in the pending sorted set. A later "Claim next" removed the
stale entry, found that the session was not pending, and returned HTTP 409 even
though the earlier individual claim had succeeded.

The fix applies at both boundaries:

1. Redis `enqueue` now atomically checks the active-claims hash before adding a
   sorted-set entry. `claim` and `claim_session` also refuse to overwrite an
   existing owner and clean up duplicate queue entries left by older code.
2. The API's generic "Claim next" path checks the session state after claiming
   an item. Missing or non-pending sessions are released and skipped, allowing
   the next valid queue item to be claimed instead of surfacing a stale-entry
   409. An assignment that genuinely races with another state update can still
   return 409; that is a real concurrency conflict, not a stale queue item.

The new Redis integration tests cover re-enqueueing a claimed session and two
volunteers concurrently claiming the same session. They are ignored in the
normal workspace run because they need Redis. Run them with the local Compose
Redis services:

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
cargo test -p sayangcare-priority-queue --lib -- --include-ignored
```

The queue tests use unique Redis keys and clean them up after successful
assertions. The command also runs the existing Redis queue integration test.

## TEST-01: 100-VU k6 lifecycle and archive validation

Keep the API reachable through the local Ingress port-forward so requests can
reach multiple replicas:

```powershell
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18081:80
```

In another terminal, first smoke-test, then run a repeatable benchmark with a
fresh run ID:

```powershell
k6 version
k6 run -e BASE_URL=http://127.0.0.1:18081 -e VUS=2 -e DURATION=10s `
  -e RUN_ID=SMOKE-20261025 tests/k6/call-lifecycle.js

k6 run -e BASE_URL=http://127.0.0.1:18081 -e VUS=100 -e DURATION=30s `
  -e RUN_ID=TEST01-20261025-01 `
  --summary-export artifacts/perf/TEST01-20261025-01-summary.json `
  tests/k6/call-lifecycle.js
```

The scenario gives each VU/iteration a distinct session ID and checks:

- incoming, turn, and hangup status **and response fields**;
- a read of the session after hangup, which exercises the API's archived
  session read path after the finish operation archives to PostgreSQL and
  removes the matching Redis version;
- the caller's exact transcript text and an assistant turn in the returned
  transcript;
- completed-lifecycle rate, archive-consistency rate, archive-read retries,
  HTTP failure rate, and request p50/p95/p99 latency.

Only the final read is retried, because it is idempotent. Replaying a turn can
duplicate transcript content, so the mutating lifecycle requests are not
automatically retried. `http_reqs` includes the archive GET (and any retries);
use completed iterations/lifecycles, not HTTP request rate, when reporting
call throughput. A successful API read-after-finish is evidence of the
archive path and transcript contents; if a direct database audit is required,
also query the PostgreSQL `sessions` row for one of the generated IDs. With
the local Compose database, for example:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "SELECT id, state, transcript->'turns' AS turns FROM sessions WHERE id LIKE 'SIM-K6-TEST01-20261025-01-%' ORDER BY created_at DESC LIMIT 5;"
```

Replace the run ID with the value used for your benchmark. The `turns` JSON
should include the exact caller text and an assistant response for each
completed sample.

The thresholds require more than 99% successful lifecycles and archive
consistency, fewer than 1% failed HTTP requests, and HTTP p95 below two
seconds. A k6 nonzero exit or a failing custom metric means the run did not
meet those thresholds; retain the JSON summary and logs rather than calling it
a pass.

## TEST-02: API pod replacement during active calls

Use the request path and k6 scenario above with a duration long enough to leave
time for pod replacement, for example three minutes. Begin the run first:

```powershell
k6 run -e BASE_URL=http://127.0.0.1:18081 -e VUS=100 -e DURATION=3m `
  -e RUN_ID=TEST02-20261025-01 `
  --summary-export artifacts/perf/TEST02-20261025-01-summary.json `
  tests/k6/call-lifecycle.js
```

While that command is actively generating calls, use a second terminal. Verify
the context and target pod before deletion; select a pod name from the output:

```powershell
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api
$deleteStarted = [DateTime]::UtcNow
kubectl --context minikube -n sayangcare delete pod API_POD_NAME
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api
$recoveryEnded = [DateTime]::UtcNow
[pscustomobject]@{
  delete_started_utc = $deleteStarted.ToString('o')
  deployment_recovered_utc = $recoveryEnded.ToString('o')
  recovery_seconds = [math]::Round(($recoveryEnded - $deleteStarted).TotalSeconds, 1)
}
```

Record the actual context, pod name, deletion and recovery timestamps, ready
replica counts, and the k6 summary. Compare lifecycle/archive consistency,
HTTP failures, `call_archive_read_retries`, and p95/p99 with a baseline run
using the same image, VUs, duration, HPA settings, and cluster resources. The
k6 read retries quantify transient archive-read recovery; the PowerShell
timestamps quantify Deployment recovery. A later healthy request alone does
not prove that every call completed or that its transcript was archived.

## TEST-05: HPA scale-up/down and latency evaluation

The existing request-rate HPA overlay targets 25 requests/second per pod. Do
not apply it until Prometheus Adapter is available and the custom metrics API
returns the SayangCare request-rate metric. The detailed setup is in the
[Phase 4 guide](./PHASE_4_KUBERNETES.md). Keep the Ingress port-forward open,
then run the evaluator from PowerShell:

```powershell
.\tests\k6\evaluate-hpa.ps1 `
  -Context minikube `
  -Namespace sayangcare `
  -BaseUrl http://127.0.0.1:18081 `
  -Vus 100 `
  -Duration 5m `
  -DownscaleTimeoutSeconds 900
```

The script runs the checked-in lifecycle benchmark, samples HPA and Deployment
state every five seconds, then watches cooldown until replicas return to the
pre-load count or the timeout is reached. It writes:

- a k6 JSON summary with HTTP p95/p99, HTTP failures, completed lifecycles, and
  archive consistency;
- a timestamped CSV of HPA current/desired/ready replicas, conditions, and
  current metrics throughout load and cooldown;
- a JSON evaluation report with first desired scale-up delay, first added-pod
  readiness delay, maximum observed replicas, and downscale delay.

The five-second polling interval limits timing precision. A missing scale-up
or a cooldown timeout is recorded in the report and should be investigated;
the runner does not alter HPA settings or force the Deployment back to its
  baseline. PowerShell prompts before starting the load; verify the context and
  namespace in that confirmation. Save the generated files with the test date, API image, HPA target,
cluster topology, and command. Compare p95/p99 and error rate against a
like-for-like baseline; do not attribute changes to autoscaling when the
workload or cluster differs.

## Before sharing results

- Include the script revision and exact command, date/time, API image, VUs,
  duration, HPA target, and cluster topology.
- Keep HTTP request throughput separate from completed call throughput.
- Report archive consistency and retry counts, not just HTTP status checks.
- Label one-node Minikube results as pod-level local evidence, not node/zone
  availability or production capacity.
- Do not use real caller, patient, Twilio, or volunteer data for these runs.
