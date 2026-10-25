# TEST-04 — Dependency recovery under active traffic

Status: **Recovery consistency passes on Windows; PostgreSQL strict availability fails.**
See [the dated results](../../docs/PHASE_5_LOCAL_RESULTS.md) for raw evidence.
After adding PostgreSQL commit checkpoints, automatic Redis promotion completed
2,149 lifecycles without HTTP errors or lost acknowledged writes. PostgreSQL
replacement retained every acknowledged create/turn/hangup. Local CNPG
smart-shutdown tuning reduced recovery from 60.995 to 27.943 s and HTTP failures
from 2.634% to 1.600%; native k6 exit 99 still fails availability.
Mock inference recovery and cross-pod alert durability passed.
The earlier 17 lost Redis records remain historical failure evidence.
See [the solution and AI results](../../docs/TEST04_AND_AI_VALIDATION.md).

Use only a test environment, never production. The new Kubernetes runners below
work with Windows PowerShell 5; the legacy concurrent Compose load helper
further below requires PowerShell 7 (`pwsh`).

## Kubernetes runners (Windows PowerShell 5)

Require k6, kubectl, a healthy three-instance CNPG cluster, three Redis nodes,
three Sentinels, three API pods, and NGINX Ingress. Commands use the explicit
`minikube` context and `sayangcare` namespace by default.

```powershell
# Inspect intended actions before confirming a test.
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres -WhatIf
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis

# Controlled promotion, NOT an automatic outage/pod-loss experiment.
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis -FaultMode sentinel-failover

# Bounded response outage; promotion is automatic, not explicitly requested.
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis `
  -FaultMode master-pause -PauseMilliseconds 60000 -Duration 120s

# Creates a synthetic escalation and replaces only its originating API pod.
.\tests\reliability\evaluate-alerts.ps1
```

Use `-KubectlPath` / `-K6Path` if executables are not on PATH. Dependency runs
own ingress port-forward 18185; the alert runner owns pod forwards 18187/18188.
They clean up only their own process IDs and never delete PVCs. Keep these
ports free. Default dependency load is 30 VUs for 60 seconds. Results are saved
under `artifacts\phase5-final\dependencies` and `artifacts\phase5-final\alerts`.
Use generated synthetic data only.

The runner logs acknowledged incoming, turn, and hangup IDs separately.
Every acknowledged create must have a durable checkpoint, every acknowledged
turn must contain the exact caller/assistant pair, and every acknowledged
hangup must have a terminal cold archive. Interrupted incoming-only calls may
remain durable until the five-minute inactivity sweep; do not require invented
turns for failed requests. Both cold and checkpoint inventories and remaining
hot keys are recorded. API read-back alone can now read a checkpoint, so the
independent SQL cold-archive check remains essential. It reports k6 availability thresholds
separately from archival consistency and observed primary change. A failed
promotion or consistency check raises an error after saving its evidence.
Availability thresholds also raise an error unless `-AllowTransientFailures`
is explicitly supplied for a recovery-only test; the failed k6 exit code and
availability flag remain in the report either way.
Some missing-cold sessions may still be recoverable in Redis; inspect retained
hot state rather than equating every failed HTTP request with data loss.

## Legacy Compose procedures

## 1. Baseline / active load

Terminal A:

```powershell
pwsh -File tests/reliability/test04-load.ps1 -Count 100 -Throttle 20 -Prefix TEST04-BASE
```

The CSV and JSON include run IDs, success/error details, throughput, and **full lifecycle** p95/p99 durations. These are not HTTP latency histograms; use Prometheus HTTP duration metrics for route-specific p95/p99. Record API and container logs. Use unique run prefixes per trial.

## 2. PostgreSQL interruption while load is running

Start the load command in terminal A with `-Prefix TEST04-PG -Count 200 -Throttle 20`. While requests are in flight, in terminal B run:

```powershell
docker compose stop postgres
# Observe retries/errors for 15-30 seconds, then restore
docker compose start postgres
docker compose exec postgres pg_isready -U sayangcare -d sayangcare
```

Alternatively `pwsh -File tests/reliability/test04-postgres.ps1 -Count 30` is a **sequential** helper, not a substitute for active concurrent traffic. Record UTC stop/restore timestamps and the time the final session is archived. After sweeper recovery:

```powershell
pwsh -File tests/reliability/test04-verify.ps1 -RunPrefix TEST04-PG
```

Check **every** run ID against PostgreSQL and Redis; no missing session may disappear from both. Check transcript turns, session versions, and duplicates. Some HTTP failures may be expected while PostgreSQL is down; they do not establish data loss. Do not claim a pass solely from HTTP 200 responses.

## 3. Redis Sentinel master failover

Start `test04-load.ps1` with `-Prefix TEST04-REDIS -Count 200`. Query Sentinel to determine the actual master:

```powershell
docker compose exec redis-sentinel redis-cli -p 26379 SENTINEL get-master-addr-by-name mymaster
```

**Check your configured master name and Sentinel port** before running this command; `mymaster`/`26379` are examples. Stop the identified master container only in the test environment, record the UTC time, observe Sentinel promotion and client reconnection, then restore the node. Run `test04-verify.ps1 -RunPrefix TEST04-REDIS`. Check missing/duplicate turns and the archive backlog. If Redis master loss causes lost acknowledged writes because of asynchronous replication, report it as a limitation rather than claiming zero loss.

## 4. Circuit-breaker fallback

Start controlled inference-enabled traffic with a deterministic workload. Capture baseline breaker-state metrics, fallback count, and HTTP latency. In a **test configuration**, disable or delay the configured inference dependency; do not edit production credentials. Observe Closed -> Open -> Half-Open -> Closed (if the configured thresholds and probing behavior permit), cached-audio fallback, and recovery after restoring the dependency. Record state transitions, fallback count, error rate, and latency. The baseline load script only covers incoming/hangup; it does **not** exercise inference turns, so use your team's existing turn/inference workload for this scenario.

## 5. FIX-03 cross-pod verification

Apply `migrations/0008_operator_alerts.sql` through normal SQLx migrations. Start two distinct API pods backed by the same PostgreSQL database. In separate terminals:

```powershell
kubectl port-forward pod/<POD_A> 8081:8081
kubectl port-forward pod/<POD_B> 8082:8081
```

Adjust the **pod container port** to match your deployment. Trigger a real alert using a supported workflow (e.g. high-risk call or volunteer handoff), then run:

```powershell
pwsh -File tests/reliability/test04-alerts.ps1 -SessionId <EXPECTED_SYNTHETIC_SESSION_ID>
```

Confirm the expected session's nonempty alert appears on both pods, replace
one pod, port-forward the replacement, and compare again. The helper requires
an expected session so two empty lists cannot pass. It compares the latest
25 IDs; for a stronger check also query PostgreSQL:

```sql
SELECT id, kind, session_id, created_at FROM operator_alerts ORDER BY created_at DESC LIMIT 25;
```

## Pass criteria

- PostgreSQL outage: no acknowledged operation is lost; retained checkpoints eventually archive after recovery, with no duplicate turns. Availability is a separate criterion and currently fails.
- Redis failover: record recovery time, HTTP failures, consistency and acknowledged-write-loss limitations.
- Circuit breaker: fallback activates on failure and recovers on restoration with measured transitions and latency.
- Alerts: same alert IDs across two replicas and after pod replacement.

**Do not mark TEST-04 passed until the actual dependency failures have been induced and measured.**
