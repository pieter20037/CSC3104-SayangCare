# Phase 5: Chaos Testing, Metrics, and Final Evaluation (Windows)

For macOS commands, see the [Mac guide](./PHASE_5_Mac.md).

This guide records the Phase 5 backlog, how to start the local Kubernetes
stack, and how to run its checks. Local test results are recorded in
[PHASE_5_LOCAL_RESULTS.md](./PHASE_5_LOCAL_RESULTS.md). The API exposes HTTP
request counters, response status-category counters, per-route request
counters/histograms, active-session/archive-backlog gauges, and circuit-breaker
state. Prometheus scraping has been verified across three API pods.
FIX-02, TEST-01, TEST-02, and TEST-05 implementation details and rerun steps
are in the [queue and load-testing guide](./QUEUE_AND_LOAD_TESTING.md).

## Milestones and Current Status

| ID | Milestone | Status | Current evidence / remaining work |
| --- | --- | --- | --- |
| **API-04** | Actix Web instrumentation | **HTTP metrics implemented and verified; remaining metrics pending** | `GET /api/v1/metrics` exposes `sayangcare_http_requests_total`, `sayangcare_http_requests_by_status_total`, and `sayangcare_http_request_duration_seconds` histogram. Verified using live HTTP requests, including a 404 response, and 14 passing API unit tests. Active-session and circuit-breaker metrics remain outstanding. |
| **OBS-01** | Prometheus scraping | **API pod scraping verified; additional metrics pending** | Prometheus successfully discovers and scrapes all three SayangCare API pods independently (`3/3 UP`). PromQL verified per-pod request counters, aggregate request rates, and HTTP status-category rates. Active Redis-session and circuit-breaker metrics are not yet exposed for scraping. |
| **OBS-02** | Grafana dashboard | **Implemented locally** | `k8s/monitoring/grafana-values.yaml` provisions a read-only dashboard with API request-rate and counter panels. Local dashboards are ephemeral. |
| **TEST-01** | k6 concurrency benchmark | **Earlier HTTP-status run recorded; stronger checks ready to rerun** | The current script also verifies response bodies and transcript retrieval after archival. Earlier summaries predate those checks; see the queue and load-testing guide. |
| **TEST-02** | Pod failover chaos test | **Earlier pod-level run recorded; consistency checks ready to rerun** | Earlier 100-VU pod deletion results are recorded, but use the current archive-consistency metrics to confirm calls and transcripts during a repeat. This is not node-level HA testing. |
| **TEST-03** | Evaluation metrics gathering | **Local evidence recorded** | Baseline, pod-loss, API/database readiness, replica scaling, and PostgreSQL/Redis pod-recovery results are in the local results report. A single-node cluster cannot establish node/zone HA. |
| **TEST-05** | HPA scaling performance evaluation | **Evaluation harness added; new timed results pending** | `tests/k6/evaluate-hpa.ps1` records scale-up/down delays and pairs HPA samples with k6 latency/error summaries. Existing HPA scale observations are not scale-delay measurements. |

The local API HPA currently has CPU, memory, and request-rate metrics enabled.
Its request-rate target is 25 requests/second per API pod.

## Requirements

- Docker Desktop with Linux containers, Minikube, `kubectl`, and Helm.
- The repository's Kubernetes manifests and the `sayangcare-api:dev` image.
- A test cluster only for pod deletion and failover experiments. The current
  Minikube cluster is single-node; these tests cannot demonstrate node-level HA.
- k6 and the checked-in `tests/k6/call-lifecycle.js` script are used for
  TEST-01/TEST-02.

Use simulated caller data only. Do not use real patient/caller data in load
tests.

## Start the Existing Local Kubernetes Stack

### Friendly dashboard launcher (recommended for daily use)

With Docker Desktop running and the Phase 4 cluster already installed, run this
from the repository root:

```powershell
.\scripts\start-dashboard.ps1 -OpenBrowser
```

It prints a SayangCare boot message, waits for the database operator,
PostgreSQL, Redis, Sentinel, and API, then checks API health, storage readiness,
and the actual dashboard page. Green check marks mean those checks succeeded.
Only then does it print **ACCESS THE DASHBOARD HERE:
http://127.0.0.1:18080/dashboard** and open your browser.
It waits for API pods to become Ready and retries the local connection if
Kubernetes briefly replaces a pod during startup.

Keep that terminal open; Ctrl+C closes only its own port-forward, not the
cluster. It targets the `minikube` context explicitly without changing your
current context. It does not install resources, change secrets, apply overlays,
build images, or start Compose. Complete the one-time setup below first.

If PowerShell blocks the script, allow local scripts in this terminal only:

```powershell
Set-ExecutionPolicy -Scope Process RemoteSigned
```

If Minikube is already running, use `-SkipMinikubeStart`. If port 18080 is
occupied, keep your existing forward or use `-Port 18081`. Do not run the
manual API port-forward below on the same port at the same time.
The launcher also checks Minikube's default Windows install location
(`C:\Program Files\Kubernetes\Minikube\minikube.exe`) if `minikube` is not on
the current terminal's `PATH`. If Minikube is installed elsewhere, reopen VS
Code after installation or add its install folder to `PATH`.
For a temporary startup check that closes its connection afterward:

```powershell
.\scripts\start-dashboard.ps1 -SkipMinikubeStart -CheckOnly
```

### Manual startup / first-time setup

Open PowerShell at the repository root:

```powershell
Set-Location "C:\Users\darre\CSC3104-SayangCare Project\CSC3104-SayangCare"
minikube start --driver=docker
kubectl config use-context minikube
kubectl get nodes
```

Wait for the CloudNativePG operator, database cluster, and application:

```powershell
kubectl rollout status deployment/cnpg-controller-manager -n cnpg-system
kubectl -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel
kubectl -n sayangcare rollout status deployment/sayangcare-api
```

For a new Minikube profile, first create the namespace and test secrets and
install the CloudNativePG operator as described in
[Phase 4 Kubernetes setup](./PHASE_4_KUBERNETES.md). Do not replace an existing
database or re-create secrets without taking a backup and following that guide's
migration instructions.

install the local Prometheus, adapter, and Grafana only if they are not already
installed:


```powershell
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update
helm upgrade --install prometheus prometheus-community/prometheus `
  --namespace monitoring --create-namespace `
  --values k8s/monitoring/prometheus-values.yaml
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter `
  --namespace monitoring `
  --values k8s/monitoring/prometheus-adapter-values.yaml
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
if (-not (kubectl --context minikube -n monitoring get secret grafana-admin --ignore-not-found -o name)) {
  $bytes = New-Object byte[] 32
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  $rng.GetBytes($bytes)
  $password = [Convert]::ToBase64String($bytes)
  kubectl --context minikube -n monitoring create secret generic grafana-admin `
    --from-literal=admin-user=admin --from-literal="admin-password=$password"
}
helm upgrade --install grafana grafana/grafana --namespace monitoring `
  --values k8s/monitoring/grafana-values.yaml
kubectl -n monitoring rollout status deployment/prometheus-server
kubectl -n monitoring rollout status deployment/prometheus-adapter
kubectl -n monitoring rollout status deployment/grafana
```

Apply the request-rate overlay. It includes the Minikube HTTP Ingress settings
as well as the request-rate metric:

```powershell
kubectl apply -k k8s/overlays/request-rate-hpa
kubectl get apiservice v1beta1.custom.metrics.k8s.io
kubectl -n sayangcare get pods,hpa
```

The Prometheus Adapter APIService should report `Available=True`. The custom
metric is available after Prometheus has scraped API traffic; initially it may
be absent until requests have been made and samples collected.

## Run the API and Inspect Metrics

If you used the launcher, your API is already forwarded. Otherwise, in one
PowerShell terminal, forward the API service:

```powershell
kubectl -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

In another terminal, check health, readiness, and the current metrics endpoint:

```powershell
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/ready
curl.exe --noproxy "*" http://127.0.0.1:18080/api/v1/metrics
```

Expected: health and readiness return HTTP 200, readiness reports Redis and
PostgreSQL as `ok`, and the metrics endpoint exposes:

- `sayangcare_http_requests_total` — cumulative HTTP request count.
- `sayangcare_http_requests_by_status_total` — HTTP requests grouped by
  response status category (`2xx`, `3xx`, `4xx`, `5xx`).
- `sayangcare_http_request_duration_seconds` — HTTP request duration
  histogram, including cumulative buckets, sum, and count.

The metrics are maintained independently by each API process.
The `/api/v1/metrics` endpoint is excluded from HTTP request instrumentation
to avoid counting Prometheus scrapes.

Active-session counts and circuit-breaker state metrics are not yet exposed.

To inspect Prometheus, use a separate terminal:

```powershell
kubectl -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
```

Open <http://127.0.0.1:19090> and run these PromQL queries:

```promql
sayangcare_http_requests_total
```

```promql
sum(rate(sayangcare_http_requests_total[2m])) by (namespace, pod)
```

The request-rate HPA's custom metric can be inspected with:

```powershell
kubectl get --raw "/apis/custom.metrics.k8s.io/v1beta1/namespaces/sayangcare/pods/*/sayangcare_http_requests_per_second"
kubectl -n sayangcare describe hpa sayangcare-api
```

### Additional HTTP metrics verification

To calculate the total request rate across all API pods:

```promql
sum(rate(sayangcare_http_requests_total{namespace="sayangcare"}[5m]))
```

To calculate request rates grouped by HTTP status category:

```promql
sum by (status) (
  rate(sayangcare_http_requests_by_status_total{namespace="sayangcare"}[5m])
)
```

To estimate the 95th percentile HTTP request duration:

```promql
histogram_quantile(
  0.95,
  sum by (le) (
    rate(sayangcare_http_request_duration_seconds_bucket{namespace="sayangcare"}[5m])
  )
)
```

Prometheus should discover three independent SayangCare API pod targets
under the `kubernetes-pods` scrape job, with all three reporting `UP`.

During local verification, Prometheus returned three per-pod request-counter
series and an aggregate request rate of approximately 0.80 requests/second.
The status-category query returned approximately 0.80 requests/second for
`2xx` and zero for the other categories during the evaluated window.

These measurements include Kubernetes health/readiness probes and are not
representative of full caller-turn or AI inference latency.
```


The Prometheus deployment uses one-hour retention and no persistent volume; it is
for local testing, not long-term evaluation data storage.
Grafana is also local-only and non-persistent. The Grafana chart currently emits
a deprecation warning; it is used here for a disposable local dashboard, not as
a production monitoring recommendation. Its anonymous viewer is available only
through a localhost port-forward:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

Open <http://127.0.0.1:13000> to view the provisioned SayangCare dashboard.

## Run a Simulated Call Lifecycle

Keep the API port-forward running. Run these steps in the same PowerShell
terminal. No live Twilio credentials are needed.

**1. Set up a unique test call:**

```powershell
$api = "http://127.0.0.1:18080/api/v1/calls"
$sid = "SIM-$([guid]::NewGuid().ToString('N'))"
```

**2. Start the call:**

```powershell
$body = @{ call_sid = $sid; from = "+6590000000"; to = "+6560000000" } | ConvertTo-Json
Invoke-RestMethod "$api/incoming" -Method Post -ContentType "application/json" -Body $body
```

**3. Record a turn:**

```powershell
$body = @{ transcript = "I have been feeling overwhelmed." } | ConvertTo-Json
Invoke-RestMethod "$api/$sid/turn" -Method Post -ContentType "application/json" -Body $body
```

**4. End the call:**

```powershell
Invoke-RestMethod "$api/$sid/hangup" -Method Post
```

Use these PowerShell-native commands rather than inline JSON with `curl.exe`
to avoid Windows PowerShell quote-escaping issues.

The dashboard polls the selected Call SID every five seconds. To follow a
session created by these PowerShell commands, paste `$sid` into the dashboard's
**Call SID** field (or open `/dashboard?sid=<Call-SID>`). The session record
will update automatically while the dashboard page is open and visible. The
dashboard does not discover arbitrary sessions by itself.

The simulator stores active session state in Redis and archives a completed
session to PostgreSQL. The browser UI is available at
<http://127.0.0.1:18080/dashboard>.

## Existing Automated Regression Tests

From the repository root, with the Rust toolchain installed:

```powershell
cargo test --workspace --locked
```

These are the group's existing tests, not the planned k6 benchmark. They cover
session transitions, risk and sentiment, archive retries/recovery, circuit
breakers and fallback, Twilio validation, telemetry, and Raft queue behavior.
Most use in-memory test doubles and do not require Compose or Minikube.

Validation on 22 October 2026: **72 passed, 0 failed, 2 ignored**.
The two ignored tests were then run against a disposable Redis 7 container:
**2 passed, 0 failed**. That container was removed afterward; no application
Redis data was used. These checks do not validate live Twilio, microphone audio,
load performance, or pod-failover behavior.

To run those Redis-dependent tests yourself, first provide an isolated test
Redis at `127.0.0.1:6379` (never point them at production/application data):

```powershell
cargo test --locked -p sayangcare-circuit-breaker -p sayangcare-priority-queue -- --ignored
```

The launcher was also verified against the running Minikube cluster in
`-CheckOnly` mode, including temporary port-forward cleanup. An occupied port
was checked to produce a red failure rather than a false dashboard-ready message.

## Run k6 and Chaos Checks

The checked-in k6 scenario creates a unique synthetic session for each
iteration, checks the HTTP status of incoming/turn/hangup, records completed
lifecycle and request metrics, and pauses each VU for one second between calls.
Use the Ingress port-forward for load tests so traffic can reach multiple API
replicas; a Service port-forward selects a single pod.

If k6 is not installed, install it using Windows Package Manager:

```powershell
winget install --id GrafanaLabs.k6 --exact --source winget
```

After installation, fully close and reopen VS Code so its terminals pick up
the updated PATH. Open a new PowerShell terminal, then check:

```powershell
k6 version
```

If `k6` is still not recognized, try the default installation path directly:

```powershell
& "C:\Program Files\k6\k6.exe" version
```

If this prints a version, k6 is installed; the issue is PATH, not a failed
installation. You do not need to reinstall it.

Start a load-balanced local Ingress port-forward:

```powershell
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18081:80
```

Run a short smoke test first:

```powershell
& "C:\Program Files\k6\k6.exe" run -e BASE_URL=http://127.0.0.1:18081 `
  -e VUS=2 -e DURATION=10s tests/k6/call-lifecycle.js
```

Then run the 100-VU benchmark and save its JSON summary:

```powershell
& "C:\Program Files\k6\k6.exe" run -e BASE_URL=http://127.0.0.1:18081 `
  -e VUS=100 -e DURATION=30s `
  --summary-export docs/phase5-local-baseline.json tests/k6/call-lifecycle.js
```

The summary includes p50 (median), p95, p99, request rate, completed call
lifecycles, archive consistency, and errors. Save the VU count, duration,
image, HPA/replica state, and cluster topology with each result. The earlier
[local results report](./PHASE_5_LOCAL_RESULTS.md) predates body/archive checks.

For TEST-02, run the same script with a separate summary file while deleting
only a pod in the explicitly selected local `minikube` cluster. In another
terminal, inspect the candidate:

```powershell
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api
```

In another terminal, select a pod name from that output and delete only that
pod:

```powershell
kubectl --context minikube -n sayangcare delete pod API_POD_NAME
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api
```

Capture the k6 failures/latencies during deletion and verify the deployment
returns to its HPA desired replica count. Run destructive failover tests only
against the disposable local cluster, never a production database.

For TEST-05, run the timed HPA evaluator after installing the request-rate
overlay and confirming the custom metric API is healthy:

```powershell
.\tests\k6\evaluate-hpa.ps1 -Context minikube -Namespace sayangcare `
  -BaseUrl http://127.0.0.1:18081 -Vus 100 -Duration 5m `
  -DownscaleTimeoutSeconds 900
```

It saves timestamped HPA replica/metric samples and a k6 summary under
`artifacts/perf/hpa`; see the [queue and load-testing guide](./QUEUE_AND_LOAD_TESTING.md)
for outputs and interpretation.

## Collect Phase 5 Evaluation Results

For each test, save:

1. Test date, cluster version/topology, API image, and test command/script
   revision.
2. VUs, test duration, total requests, throughput, p50/p95/p99 latency, and
   error rate.
3. HPA desired/current replicas and CPU/memory/request-rate observations.
4. Start and recovery timestamps for the pod-failure test, plus session/data
   consistency checks before and after.
5. Limitations, including the single-node Minikube topology and whether the
   test used simulator traffic or real telephony.

Do not present HTTP request rate as call throughput unless the test counts
completed call lifecycles separately.

## Stop the Local Forwards

Press `Ctrl+C` in each port-forward terminal. Keep Minikube and its PVCs running
if you want to continue later. To stop the cluster without deleting its data:

```powershell
minikube stop
```

## Session Handoff: Completed Work and Current State

**Last updated:** 2026-10-23  
**Checkpoint:** Local Phase 4/5 verification is complete; all project services
were stopped without deleting the cluster or its data.

### Work completed

- Installed Minikube 1.39.0, Helm 4.3.1, and k6 2.2.0. Started a Docker-backed
  single-node Minikube cluster with Kubernetes 1.37.0, enabled storage,
  Metrics Server, and Ingress, and deployed the SayangCare API, PostgreSQL
  (CloudNativePG), Redis/Sentinel, Prometheus, Prometheus Adapter, and Grafana.
- Created local test configuration and test-only secrets, built and loaded the
  API image, and verified the dashboard, health/readiness, monitoring, and
  request-rate HPA.
- Ran synthetic 100-VU baseline and API-pod-deletion load tests, plus
  PostgreSQL primary-promotion and Redis Sentinel-promotion checks. The
  measurements and limitations are in
  [PHASE_5_LOCAL_RESULTS.md](./PHASE_5_LOCAL_RESULTS.md).
- Ran the Rust workspace tests: 72 passed, 0 failed, and 2 were ignored in
  that run. See the test notes earlier in this guide for the subsequent
  isolated Redis test results.

### Additional workspace files

The application’s Rust source code was not changed. Supporting Phase 5 test,
monitoring, and documentation files were added or updated:

- Added `tests/k6/call-lifecycle.js` for synthetic call lifecycle load tests.
- Added `k8s/monitoring/grafana-values.yaml` for the local Grafana dashboard.
- Added `docs/PHASE_5_LOCAL_RESULTS.md` and the saved k6 summaries
  `docs/phase5-local-baseline.json` and
  `docs/phase5-local-api-failover.json`.
- Updated `scripts/start-dashboard.ps1` to find Minikube in its default
  Windows install location when it is missing from the current terminal's
  `PATH`, wait for API pods to become Ready, and retry transient port-forward
  exits during startup. The launcher was rerun successfully; health, readiness,
  and dashboard endpoints all returned HTTP 200 before the services were
  stopped again.
- Updated this guide, `docs/PHASE_5_Mac.md`, and `docs/README.md`.

These are local workspace changes and were not committed. The local `.env` and
the test environment were retained; credentials are placeholders, not real
telephony or inference-provider credentials.

### Where work stopped

At the user's request, the native API process, Kubernetes port-forwards, the
Minikube cluster, and the project's Docker Compose PostgreSQL and Redis/Sentinel
containers were stopped. The dashboard, Grafana, Prometheus, and load-test
ports (8081, 18080, 18081, 13000, and 19090) were verified not to be listening.
Minikube reports its host, kubelet, and API server stopped.

The Minikube profile, containers, persistent data, local `.env`, and workspace
files were **not deleted**; Minikube, Helm, and k6 were **not uninstalled**.
The cluster and Compose services must be started again before opening the
dashboard. This is a local single-node test setup, not production or
multi-node/high-availability validation.
