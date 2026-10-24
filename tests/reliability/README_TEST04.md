# TEST-04 — Dependency recovery under active traffic

Status: **implementation and test harness prepared; execution results must be collected on your local deployment.** Use only a disposable environment. Requires PowerShell 7 (`pwsh`), Docker, running API, and a working PostgreSQL/Redis Sentinel deployment. Keep a separate terminal running the API. Do not run faults on production.

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
pwsh -File tests/reliability/test04-alerts.ps1
```

Confirm the alert appears on both pods, replace one pod, port-forward the replacement, and compare again. The helper compares the latest 25 IDs; for a stronger check also query PostgreSQL:

```sql
SELECT id, kind, session_id, created_at FROM operator_alerts ORDER BY created_at DESC LIMIT 25;
```

## Pass criteria

- PostgreSQL outage: unarchived terminal sessions remain in Redis and eventually archive after recovery; no missing records or duplicate turns.
- Redis failover: record recovery time, HTTP failures, consistency and acknowledged-write-loss limitations.
- Circuit breaker: fallback activates on failure and recovers on restoration with measured transitions and latency.
- Alerts: same alert IDs across two replicas and after pod replacement.

**Do not mark TEST-04 passed until the actual dependency failures have been induced and measured.**
