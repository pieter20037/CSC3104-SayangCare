# Phase 6 (Windows): What to run, what to fix, and what to submit

Updated 3 November 2026. Run PowerShell from the repository root.
This is a follow-up to Phase 5, not a request to rebuild completed features.

**Agreed demonstration scope:** the professor approved the dashboard instead of
live Twilio calls. Demonstrate synthetic caller turns, escalation and volunteer
ownership. Live speech/audio/callback/outbound delivery remains an explicit
limitation, not a submission blocker or a claimed test pass.

## Final milestone: ownership generations

The fencing enhancement keeps Redis waiting order and adds generation-guarded
ownership in both PostgreSQL-only and Raft modes. Read
[the fencing runbook](RAFT_FENCING.md) before upgrading or running faults.
Transfer/resolve now require an explicit generation and request ID; the updated
dashboard sends them. Existing images must be rebuilt/deployed to use this API.
Do not delete legacy data to bypass a migration/reconciliation error.

```powershell
cargo test --workspace --locked
.\scripts\test-raft-fencing.ps1
```

The comparison requires both configured deployments; fault injection is
separately opt-in. New fencing results are not supplied by the older Phase 5
reports below.

## 1. Start here if you are lost

1. For a normal demonstration, follow sections 3 and 4 only. No phone call,
   Groq key, load test or pod deletion is required.
2. For regression testing, follow section 5.
3. For the remaining engineering work, follow sections 6-8 in order.
4. For the professor meeting, read [consultation.md](consultation.md).

Do not run all experiments at once. Use synthetic callers only. Fault tests
interrupt this test cluster and may interrupt the dashboard. Tell teammates
before starting. Do not delete PVCs, run volume cleanup, force-delete database
pods, expose credentials, or use real caller data.

## 2. What is already working?

| Item | Latest verified Windows result | Meaning |
| --- | --- | --- |
| TEST-01 baseline | 100 VUs / 30 s; 2,100 lifecycles; p95/p99 360.25/830.41 ms; native exit 0; zero HTTP failures; SQL verified | Passed on fencing revision |
| TEST-02 API pod deletion | 100 VUs / 60 s; 4,410 lifecycles; p95/p99 325.80/651.11 ms; native exit 0; zero HTTP failures; SQL verified; replacement Ready 8.903 s | Passed |
| Redis automatic promotion | 20 VUs / 120 s; 2,208 lifecycles; promotion/readiness 8.157 s; no HTTP failures or missing acknowledged operations | Passed |
| TEST-04 PostgreSQL replacement | Recovery 18.662 s; HTTP failures 0.609%; all 1,958 acknowledged creates/turns/hangups preserved; native exit 99 | Recovery passes; strict availability still fails |
| TEST-05 HPA | 100 VUs / 120 s; 9,248 lifecycles; 3 -> 10 -> 3 replicas; zero HTTP failures; p95/p99 281.64/507.44 ms; native exit 0; SQL verified | Scaling, load gates and consistency pass |
| PERF-02 | Atomic terminal persistence, four actual node CPUs, 75-second API keep-alive; pool/deadlines/durability unchanged | Previous pool/latency gaps no longer reproduced in final baseline/HPA workload |
| Real Groq sample | Ten successful sentiment/reply pairs; no audio fallback; caller median 1.13 s, maximum 1.44 s | Small live smoke, not capacity evidence |

Latest evidence is in [the current-revision evidence directory](../artifacts/phase6-current/).
The deployed image is `sayangcare-api:evaluated-20261103`; source/image hashes
are recorded there. Final HPA desired scale-up took 50.5 s, additional Ready
61.7 s and full cooldown 502.6 s. Exact-ID SQL auditing matched all 9,248
acknowledged IDs at every stage; no pending work or version mismatch remained.
The PostgreSQL follow-up sweep archived all 1,959 committed checkpoints, including
unacknowledged abandoned work. It does not erase the real request failures.
Earlier failed runs remain in [the historical evaluation](TEST04_AND_AI_VALIDATION.md)
and the current evidence directory; they are not final passes.

### What changed, without weakening acceptance

- Terminal transition, archive and checkpoint retirement now share one database
  transaction rather than three synchronous commits; rollback/replay tests pass.
- Docker capped Minikube at two CPUs while Kubernetes advertised 16. The actual
  quota is now four CPUs with the same 3,900-MiB memory and retained volumes.
  HPA adds pods, not physical host capacity. Recheck after a Minikube restart:
  `docker inspect minikube --format '{{.HostConfig.NanoCpus}}'` should report
  `4000000000` for this evaluated configuration. The existing profile could not
  be resized by `minikube start`; no cluster deletion was performed.
- Actix keep-alive is 75 seconds, longer than the verified 60-second ingress
  upstream timeout. The earlier mismatch caused a recorded incoming 502;
  a real six-second-idle connection regression now passes.
- Pool size three, two-second acquisition timeout, required synchronous replica
  acknowledgment, WAL durability and the original k6 thresholds were retained.

Workspace validation: **113 passed, seven ignored, zero failed**; relevant
checkpoint/ownership database integrations passed explicitly. Code and CPU
changes are separate interventions; do not attribute all gains to one algorithm.

## 3. Open the existing program

Start Docker Desktop and wait for its engine. On this computer:

```powershell
Set-Location "C:\Users\darre\CSC3104-SayangCare Project\CSC3104-SayangCare"
docker version
.\scripts\start-dashboard.ps1 -OpenBrowser
```

Keep this terminal open. Open <http://127.0.0.1:18080/dashboard>.
Ctrl+C closes the launcher's own forward, not the cluster. If the port is
occupied, use `-Port 18081` and open the printed URL instead.

If scripts are blocked, allow local scripts in this terminal only:

```powershell
Set-ExecutionPolicy -Scope Process RemoteSigned
```

**New teammate/new cluster:** complete [Phase 4 setup](PHASE_4_KUBERNETES.md)
first. Your checkout does not contain another person's database, images,
cluster or secrets. Do not copy the current private configuration into Git.
Do not start Compose alongside this Kubernetes workflow.

In a second terminal, inspect health (all kubectl commands explicitly select
the test cluster):

```powershell
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n sayangcare get cluster sayangcare-db
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/ready
```

Expected: API and PostgreSQL have three Ready instances at rest, Redis and
Sentinel three each, health/readiness HTTP 200. HPA may temporarily leave more
API pods after load. Wait for cooldown rather than forcing replica counts.

### Deploy code/configuration changes, only when needed

```powershell
docker build -t sayangcare-api:dev .
if ($LASTEXITCODE -ne 0) { throw 'Build failed' }
minikube image load sayangcare-api:dev --profile minikube
if ($LASTEXITCODE -ne 0) { throw 'Image load failed' }
kubectl --context minikube apply -k k8s\overlays\request-rate-hpa
if ($LASTEXITCODE -ne 0) { throw 'Apply failed' }
kubectl --context minikube -n sayangcare rollout restart deployment/sayangcare-api
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=180s
```

This assumes Prometheus Adapter and the Phase 4 resources already exist.
Review manifest changes before applying; do not use this as an unreviewed reset.
The API runs SQLx migrations at bootstrap, including the checkpoint migration.
Back up valuable data before schema updates. A missing migration is not fixed
by deleting the database.

## 4. What to click and show

- Open the Call Lab/dashboard and run a simulated lifecycle. Explain incoming,
  conversation turn, hangup and transcript inspection.
- Use the synthetic crisis/escalation workflow to show queue priority and
  volunteer claiming. The displayed risk is heuristic, not clinical diagnosis.
- Explain operator alerts are shared PostgreSQL data, not owned by one API pod.
- Simulation makes no real phone call. Live Twilio is outside the agreed demonstration scope.

For monitoring, open separate terminals and leave each forward running:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:80
```

Open Grafana <http://127.0.0.1:13000> (local read-only viewing) and Prometheus
<http://127.0.0.1:19090>. Show throughput, errors, latency, sessions/backlog,
breaker states and replicas. Grafana storage is ephemeral; provisioning is in
[the repository](../k8s/monitoring/grafana-values.yaml). Redis hot-key counts
are projections, not the authoritative durable-session inventory.

## 5. Repeat known-good regression tests

Requires host Rust for Cargo tests and k6/kubectl on PATH for runners:

```powershell
cargo test --workspace --locked
.\tests\k6\test-recorded-process.ps1
.\tests\k6\evaluate-lifecycle.ps1 -Mode baseline -Port 18189 -Vus 100 -Duration 30s
.\tests\k6\evaluate-lifecycle.ps1 -Mode pod-failover -Port 18189 -Vus 100 -Duration 60s
```

The second lifecycle command deliberately deletes one API pod after ten seconds.
Preview it with `-WhatIf`. Both lifecycle runners own their Ingress forwards
and run independent SQL archive/checkpoint audits. Port 18189 must be free.
They prompt before execution; read the prompt and confirm only the test cluster.

If executables are not on PATH, add these parameters, replacing paths as needed:

```powershell
-KubectlPath 'C:\ProgramData\chocolatey\lib\kubernetes-cli\tools\kubernetes\client\bin\kubectl.exe' -K6Path 'C:\Program Files\k6\k6.exe'
```

Reports: `artifacts\phase5-final\lifecycle-checkpoint\*-report.json`.
The directory name remains Phase 5 for compatibility. Read `k6_exit_code`,
`error`, `consistency_passed`, `audit`, HTTP latency and replacement fields.
**Do not classify a null code or absent report as a pass.** Exit 99 means k6
thresholds failed, not that the script is missing.

Latest workspace run: 100 passed, five ignored. The separate PostgreSQL
integration test requires private `TEST_DATABASE_URL`; never paste it into
chat or commands containing a literal password. Ignored tests are not passes.

## 6. PERF-02 follow-up: diagnose the database bottleneck first

Owner suggestion: Lucas/Darren with Li Qi. Do not enlarge every pool blindly.
Pool size three and HPA maximum 20 project:

`(20 + 1 surge + 1 terminating) * 3 + 16 reserved = 82 connections`

The recorded primary limit was 100. Verify the current value, not an assumption:

```powershell
$primary = kubectl --context minikube -n sayangcare get cluster sayangcare-db -o jsonpath='{.status.currentPrimary}'
kubectl --context minikube -n sayangcare exec $primary -c postgres -- psql -U postgres -d sayangcare -c "SHOW max_connections;"
kubectl --context minikube -n sayangcare exec $primary -c postgres -- psql -U postgres -d sayangcare -c "SELECT application_name,state,count(*) FROM pg_stat_activity GROUP BY application_name,state;"
kubectl config current-context
```

Only if the current context is `minikube` (the capacity helper uses the current
context), run:

```powershell
.\scripts\check-pool-capacity.ps1 -DatabaseMaxConnections 100
```

Replace 100 with the observed limit. This is a connection-budget projection,
not proof the database can sustain the workload. Inspect pool occupancy,
query/transaction duration, CPU/memory and timeout logs during section 7.
Review query work, scheduling and admission before choosing pool/HPA/database
changes. Change one factor at a time; record before/after and preserve deadlines,
versioned commits, synchronous replication and explicit errors.

## 7. TEST-05 follow-up: repeat sustained load and full cooldown

Keep an Ingress forward in terminal A:

```powershell
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18193:80
```

Terminal B, after readiness HTTP 200 and stable starting replicas:

```powershell
curl.exe --noproxy "*" -i http://127.0.0.1:18193/api/v1/ready
.\tests\k6\evaluate-hpa.ps1 -BaseUrl http://127.0.0.1:18193 -Vus 100 -Duration 120s -PollSeconds 5 -DownscaleTimeoutSeconds 900 -OutputDirectory artifacts\phase6\hpa
```

Budget up to about 17 minutes (two-minute load plus up to 15-minute cooldown).
Do not delete pods or run another load during this experiment. The runner
does not own this manually started forward; Ctrl+C terminal A afterward.

Inspect `*-evaluation.json`, `*-k6-summary.json`, logs and samples in
`artifacts\phase6\hpa`. Record first desired increase, first additional Ready,
maximum replicas and downscale time. Exit 0 does not by itself prove scaling:
also require `scale_up_observed` and `downscale_observed`.

Existing k6 gates: lifecycle success >99%, **zero failed lifecycle/check steps**,
archive-read consistency >99%, HTTP failure rate <1%, HTTP p95 <2,000 ms.
The stricter zero-failure gate can fail even if the <1% HTTP gate passes.
API read-back can use a checkpoint; independent cold-archive SQL evidence is
still required. For a failed workload audit only successful acknowledged turns,
not invented turns on requests that failed. See the exact-ID audit procedure
in [TEST-04 documentation](../tests/reliability/README_TEST04.md).

## 8. TEST-04 follow-up: PostgreSQL interruption

Run separately, with three healthy database instances and no other load:

```powershell
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres -FaultMode pod-replacement -Vus 20 -Duration 120s -RecoveryTimeoutSeconds 180 -AllowTransientFailures
```

It prompts, then deletes the current primary pod (not its PVC). It owns
Ingress port 18185. Allow several minutes for workload/recovery/audit.
`-AllowTransientFailures` permits recovery evaluation to finish despite k6
failure; it **does not** make availability pass. Read both
`recovery_criteria_passed` and `availability_thresholds_passed`, plus the actual
`k6_exit_code`, in `artifacts\phase5-final\dependencies\*-report.json`.

Local smart shutdown is five seconds; the full stop budget remains 90 seconds
and synchronous replica acknowledgment remains required. Do not disable either
to improve results. Planned maintenance should switch primary before deletion.
This experiment is graceful pod replacement, not a machine crash/partition.

Other regressions, not needed for every meeting:

```powershell
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis -FaultMode master-pause -PauseMilliseconds 60000 -Vus 20 -Duration 120s -AllowTransientFailures
.\tests\reliability\evaluate-alerts.ps1
```

The Redis pause expires automatically. Do not manually force promotion in the
automatic-promotion experiment. Restore/check all three instances after faults.

## 9. AI testing is a separate, bounded experiment

The large k6 runs are simulator tests, not 100 simultaneous real Groq calls.
Do not enable paid inference or real outbound dispatch for load testing.

With the existing private virtual environment, host Rust and configured test DB:

```powershell
cargo build -p sayangcare-api --locked
.\.venv\Scripts\python.exe -B scripts\test_perf_benchmark.py
.\scripts\run-ai-evaluation.ps1 -Mode MockRecovery
```

The wrapper obtains test database access in memory from Kubernetes. It needs
owned ports 18190-18192 free; do not run concurrent AI experiments.
For an approved small live sample, use the secure configuration procedure in
[AI validation](TEST04_AND_AI_VALIDATION.md), then:

```powershell
.\scripts\run-ai-evaluation.ps1 -Mode Real -RealAiCalls 3
```

This contacts Groq and consumes quota (normally two provider operations per
turn). Do not share keys; do not put them in the example configuration. No new
key is necessary for a dashboard-only demonstration.

## 10. Phase 6 checklist and evidence handoff

- [x] TEST-04 follow-up: reduced observed interruption and documented remaining strict availability failure; zero-error failover is not achieved.
- [x] PERF-02 follow-up: final baseline/HPA pass within unchanged connection capacity and deadlines.
- [x] TEST-05 follow-up: final 100-VU rerun passes load gates, scale-up, full cooldown and SQL consistency.
- [x] REG-01: workspace, relevant integrations, baseline, pod-loss and dependency faults repeated.
- [ ] DATA-06/07: checkpoint retention/privacy policy and projection reconciliation.
- [x] TEST-03/DOC-01: final measured results, graph, reproducible commands and report updated; team submission review remains.
- [ ] OBS-03: retain dashboard provisioning/screenshots; decide whether persistence is needed.
- [x] TEL-06 scope decision: professor-approved dashboard demonstration; document unvalidated live telephony as a limitation.
- [ ] If required: multi-node failure testing, off-cluster backup/restore and public TLS/access control.

For each experiment save workload, code/image/config version, native code,
start/fault/recovery times, latencies, errors, exact-ID SQL audit and screenshot.
The filename time is often UTC; screenshot/terminal times can be Singapore time.
Capture failed runs too. Do not include secrets, caller data or Word lock files.

Troubleshooting:

| Symptom | Action |
| --- | --- |
| Port occupied | Choose another documented port/parameter; never kill an unknown process |
| Ready 503 | Inspect PostgreSQL/Redis/pods; health 200 alone does not mean dependencies work |
| Pending API pod | Inspect scheduling/resources; wait for container status, not assumed readiness |
| Pool timeout | Preserve logs and workload; follow section 6, not random pool increases |
| HPA does not downscale | Check custom metrics availability, cooldown and errors before declaring success |
| Missing k6/report | Fix the harness; do not classify absence as a service pass |
| Final gates fail | Report measured failure; do not silently weaken thresholds |
