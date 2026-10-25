# Phase 6 (macOS): Start the demo and reproduce tests honestly

Updated 26 October 2026. Commands below use macOS Terminal (zsh/bash).
Recorded results are from Windows single-node Minikube, **not Mac validation**.
These Mac procedures have not been executed on a Mac in this session.

## 1. The simple plan

1. Start the existing cluster and dashboard (sections 2-3).
2. For the meeting, stop there and read [consultation.md](consultation.md).
3. For engineering work, use the native k6 workflow below or pair with the
   Windows owner for automated fault/HPA/SQL evidence.

**Do not run the Windows fault runners unchanged on macOS.** They use
`curl.exe`, `NUL`, Windows executable defaults and Windows process arguments;
installing PowerShell alone does not make them portable. The Windows AI wrapper
also defaults to `.venv\Scripts\python.exe` and an `.exe` binary.
Portable automation is future work, not an already verified Mac feature.

The [Windows guide](PHASE6_Windows.md) contains the complete Phase 6 backlog
and acceptance definitions. Do not rebuild existing functionality.

## 2. Start your own cluster

Prerequisites: Docker Desktop for your Mac architecture, kubectl, Minikube,
Helm, k6; Rust only for host Cargo tests. Install missing CLI tools with
Homebrew if needed:

```bash
brew install minikube kubectl helm k6
```

Use your actual checkout directory:

```bash
cd "$HOME/CSC3104-SayangCare"
open -a Docker
# Wait for Docker Desktop's engine before continuing.
docker version
minikube start --driver=docker
kubectl --context minikube get nodes
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n sayangcare get cluster sayangcare-db
```

If the namespace/operator/resources do not exist, follow
[Phase 4 first-time setup](PHASE_4_KUBERNETES.md) and
[monitoring setup](PHASE_5_Mac.md) first. Do not reset/recreate a teammate's
database or secrets. A checkout does not contain their Minikube cluster.
Build the image on your Mac so its architecture matches the cluster.

After deliberate source/configuration changes, not on every startup:

```bash
docker build -t sayangcare-api:dev . &&
minikube image load sayangcare-api:dev --profile minikube &&
kubectl --context minikube apply -k k8s/overlays/request-rate-hpa &&
kubectl --context minikube -n sayangcare rollout restart deployment/sayangcare-api &&
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=180s
```

The overlay assumes monitoring/Adapter are installed. API bootstrap applies
SQLx migrations, including durable session checkpoints. Back up valuable data
before migration. Do not run Compose and Kubernetes stacks together for tests.

## 3. Dashboard and monitoring: separate terminals

Terminal A (leave open):

```bash
kubectl --context minikube -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

Terminal B:

```bash
curl --noproxy '*' -i http://127.0.0.1:18080/api/v1/health
curl --noproxy '*' -i http://127.0.0.1:18080/api/v1/ready
open http://127.0.0.1:18080/dashboard
```

Expected both HTTP 200. Use simulated lifecycle/transcript and escalation/claim
workflows. No phone number or live AI key is needed for the simulator demo.
The API Service forward is suitable for browsing, **not API-pod fault testing**:
it binds one pod and can break if that pod is replaced.

For Grafana, terminal C:

```bash
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

Open <http://127.0.0.1:13000>. Prometheus in another terminal:

```bash
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:80
```

Open <http://127.0.0.1:19090>. Ctrl+C each owned forward when finished.
If a port is occupied, choose a different local port; do not kill unknown
processes. Grafana is read-only anonymously on the local forward and storage is
ephemeral. Redis session counts measure hot projections, not durable authority.

## 4. Portable baseline with an explicit native exit code

Use a test cluster, synthetic callers, no other load/fault experiment.
Confirm the deployed API is in simulator mode rather than sending live Groq
or Twilio requests. Wait for healthy/stable API/DB/Redis/Sentinel.

Terminal A: use the Ingress controller, not the API Service:

```bash
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18189:80
```

Terminal B, run from repository root. Keep this terminal's variables for SQL:

```bash
curl --noproxy '*' --fail http://127.0.0.1:18189/api/v1/ready
RUN="MAC-BASE-$(date -u +%Y%m%dT%H%M%SZ)"
OUT="artifacts/phase6/mac/$RUN"
mkdir -p "$OUT"
# Do not use "set -e" here: capture failure before deciding what it means.
k6 run -e BASE_URL=http://127.0.0.1:18189 \
  -e VUS=100 -e DURATION=30s -e RUN_ID="$RUN" \
  -e RECORD_SESSION_IDS=true \
  --summary-export "$OUT/k6-summary.json" tests/k6/call-lifecycle.js \
  >"$OUT/k6.stdout.log" 2>"$OUT/k6.stderr.log"
K6_EXIT=$?
printf '%s\n' "$K6_EXIT" > "$OUT/k6-exit-code.txt"
cat "$OUT/k6.stdout.log"
printf 'Native k6 exit: %s\n' "$K6_EXIT"
```

Exit 0: thresholds passed. Exit 99: thresholds failed. Other codes may indicate
a harness/execution problem; inspect stderr. Never discard a failed run.
Load results include all HTTP requests; caller-turn latency is a different
metric. Baseline pass also requires the independent SQL audit below.

## 5. Independent SQL archival audit

Terminal B, after k6 finishes:

```bash
PRIMARY=$(kubectl --context minikube -n sayangcare get cluster sayangcare-db \
  -o jsonpath='{.status.currentPrimary}')
kubectl --context minikube -n sayangcare exec "$PRIMARY" -c postgres -- \
  psql -U postgres -d sayangcare -At -c "
SELECT json_build_object(
 'rows',count(*),'distinct_ids',count(DISTINCT id),
 'invalid_transcripts',count(*) FILTER (
   WHERE jsonb_array_length(transcript->'turns') IS DISTINCT FROM 2
   OR transcript->'turns'->0->>'speaker' IS DISTINCT FROM 'caller'
   OR transcript->'turns'->0->>'text' IS DISTINCT FROM 'I have been feeling overwhelmed by a busy day.'
   OR transcript->'turns'->1->>'speaker' IS DISTINCT FROM 'assistant'),
 'nonterminal',count(*) FILTER (WHERE state <> 'completed'),
 'checkpoints',(SELECT count(*) FROM session_checkpoints
   WHERE starts_with(id,'SIM-K6-$RUN-')),
 'pending',(SELECT count(*) FROM session_checkpoints
   WHERE starts_with(id,'SIM-K6-$RUN-') AND NOT archived))
FROM sessions WHERE starts_with(id,'SIM-K6-$RUN-');
" > "$OUT/sql-audit.json"
SQL_EXIT=$?
printf '%s\n' "$SQL_EXIT" > "$OUT/sql-exit-code.txt"
cat "$OUT/sql-audit.json"
```

For a clean baseline: rows = distinct IDs = checkpoints = k6 iterations;
invalid transcripts, nonterminal and pending = 0; SQL exit = 0.
Read-back through the API alone may read a checkpoint, not the cold archive.
On a fault/failed workload, incoming-only calls can legitimately lack turns:
compare **exact ACK IDs**, not only aggregate counts. The Windows dependency
runner implements create/turn/hangup-specific auditing; use its evidence for
the formal fault evaluation until a portable equivalent is tested.

## 6. Manual API-pod failover (optional, disruptive)

Use the same Ingress forward. Ensure at least three Ready API pods. Create a
new `RUN`/`OUT` as above (e.g. `MAC-POD-<UTC timestamp>`), then start the same
k6 command with `DURATION=60s` in terminal B. Do not overlap with another test.

Terminal C: select ONE Ready, non-terminating API pod from:

```bash
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api -o wide
```

After ten seconds of active k6 load, set the exact name you selected:

```bash
POD='replace-with-one-ready-api-pod-name'
kubectl --context minikube -n sayangcare get pod "$POD" \
  -o jsonpath='{.metadata.name}{" "}{.metadata.uid}{"\n"}'
date -u '+Fault UTC: %Y-%m-%dT%H:%M:%SZ'
kubectl --context minikube -n sayangcare delete pod "$POD" --wait=false
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api -w
```

Record fault UTC, old UID, new UID and first observation of a Ready replacement
and full desired availability. Ctrl+C stops the watch, not the pods. Save k6's
exit immediately in terminal B as in section 4 and perform SQL auditing.
These manual notes are not equivalent to the Windows automatic timing report.
Prefer the verified Windows runner for the formal benchmark if unsure.

## 7. TEST-05: Mac HPA measurement without pretending automation exists

For an independent Mac trial, use a unique `RUN`/`OUT`, the Ingress forward and
the same k6 command with `DURATION=120s`. In a separate terminal **before load**:

```bash
kubectl --context minikube -n sayangcare get hpa sayangcare-api -o yaml
kubectl --context minikube -n sayangcare get deployment sayangcare-api -o wide
```

Record initial current/desired/Ready replicas and UTC time. During load and for
up to 15 minutes after it ends, sample with a timestamp:

```bash
while true; do
  date -u '+%Y-%m-%dT%H:%M:%SZ'
  kubectl --context minikube -n sayangcare get hpa sayangcare-api
  kubectl --context minikube -n sayangcare get deployment sayangcare-api
  sleep 5
done
```

Redirect to a file in your run directory if collecting formal evidence. Stop
with Ctrl+C only after observing the return to the initial stable replica count
or recording a timeout. Do not declare downscale unless scale-up occurred.
Record first desired increase, first additional Ready, max counts, load-end
UTC and stable downscale UTC. Preserve k6 exit, latency and error results too.

For automatically computed timings and regression-tested native exit handling,
run [the Windows HPA procedure](PHASE6_Windows.md#7-test-05-follow-up-repeat-sustained-load-and-full-cooldown)
on the Windows test machine. This Mac manual sampling procedure does not
automatically compute per-replica-phase latency or exact recovery criteria.

## 8. Remaining work and how to avoid getting stuck

| Follow-up | What to do | What not to claim |
| --- | --- | --- |
| PERF-02 pool timeouts | Measure pool wait/query duration and DB capacity; review work/admission/HPA budget before changes | A bigger pool is automatically safe |
| TEST-04 PostgreSQL interruption | Use the Windows dependency runner with the owner; repeat after a justified change, preserving sync durability | Recovery with HTTP errors means zero downtime |
| TEST-05 load gates | Repeat sustained load/cooldown after changes; retain native exit and exact SQL evidence | Observed scaling means the latency/error gates passed |
| Retention/projections | Agree checkpoint retention/privacy policy and stale Redis reconciliation | Redis projection counts equal all durable sessions |
| Final report | Add Mac evidence only after actually executing it; screenshots, table and layout review | Windows measurements are Mac results |
| TEL-06 | Professor-approved, consenting live voice/handoff/outbound test | Simulator or Groq-only tests validate Twilio audio |
| Extended scope | Ask about multi-node, backups/restore, public TLS/security | Single-node pod replacement proves node/zone HA |

Existing connection projection: `(20 + 1 surge + 1 terminating) * 3 + 16 = 82`
against a previously measured 100-connection primary. Verify local limits and
resource capacity; this arithmetic is not a throughput guarantee.

Optional host code tests, if Rust is installed:

```bash
cargo test --workspace --locked
```

Expected baseline from Windows was 100 passed/five ignored; compare your actual
result. Do not treat ignored environment-dependent tests as passes.
For AI testing, use the existing Windows wrapper with the owner rather than
inventing credentials/forwards on Mac. Live samples consume provider quota.

## 9. Finish safely

Stop your manual forwards and watches with Ctrl+C. Confirm API/Redis/Sentinel
and DB are healthy; wait for HPA cooldown. Keep reports and screenshots, not
secrets or personal caller data. Never use `down -v` or delete PVCs to tidy tests.
Read [the consultation brief](consultation.md) for what to present and ask.
