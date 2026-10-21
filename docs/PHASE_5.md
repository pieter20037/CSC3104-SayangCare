# Phase 5: Chaos Testing, Metrics, and Final Evaluation

This guide records the Phase 5 backlog, what is already available, and how to
start the current local Kubernetes stack and run the checks that exist today.
The Phase 5 performance and observability work is not all implemented yet; do
not report planned tests as completed.

## Milestones and Current Status

| ID | Milestone | Status | Current evidence / remaining work |
| --- | --- | --- | --- |
| **API-04** | Actix Web instrumentation | **Partial** | `GET /api/v1/metrics` exposes the process-local `sayangcare_http_requests_total` counter. Latency histograms, HTTP error metrics, active session metrics, and breaker metrics are not currently exposed. |
| **OBS-01** | Prometheus scraping | **Partial** | The local Prometheus and Prometheus Adapter setup scrapes API pod counters and publishes `sayangcare_http_requests_per_second` for the request-rate HPA. Scrapes/exports for active Redis sessions and circuit-breaker state are still needed. |
| **OBS-02** | Grafana dashboard | **Not done** | There is no checked-in Grafana deployment or dashboard configuration yet. |
| **TEST-01** | k6 concurrency benchmark | **Not done** | There is no checked-in k6 scenario yet. Create one that performs the full call lifecycle before running a formal 100+ VU benchmark. |
| **TEST-02** | Pod failover chaos test | **Partial** | API pod replacement, Redis Sentinel failover, and PostgreSQL primary-pod failover were tested separately. The requested mid-call API pod termination during a k6 run remains to be tested. |
| **TEST-03** | Evaluation metrics gathering | **Partial** | Earlier Phase 4 checks recorded recovery and scaling observations. Phase 5 still needs a repeatable benchmark, collected throughput/latency/error results, recovery measurements, and consistency results in a final evidence table. |

The local API HPA currently has CPU, memory, and request-rate metrics enabled.
Its request-rate target is 25 requests/second per API pod.

## Requirements

- Docker Desktop with Linux containers, Minikube, `kubectl`, and Helm.
- The repository's Kubernetes manifests and the `sayangcare-api:dev` image.
- A test cluster only for pod deletion and failover experiments. The current
  Minikube cluster is single-node; these tests cannot demonstrate node-level HA.
- k6 is needed for TEST-01/TEST-02, but a project k6 script must be written
  before `k6 run` can execute the call-lifecycle benchmark.

Use simulated caller data only. Do not use real patient/caller data in load
tests.

## Start the Existing Local Kubernetes Stack

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

Install the local Prometheus and adapter only if the `monitoring` namespace is
not already installed:

```powershell
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update
helm upgrade --install prometheus prometheus-community/prometheus `
  --namespace monitoring --create-namespace `
  --values k8s/monitoring/prometheus-values.yaml
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter `
  --namespace monitoring `
  --values k8s/monitoring/prometheus-adapter-values.yaml
kubectl -n monitoring rollout status deployment/prometheus-server
kubectl -n monitoring rollout status deployment/prometheus-adapter
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

In one PowerShell terminal, forward the API service:

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
PostgreSQL as `ok`, and metrics include `sayangcare_http_requests_total`.
This endpoint currently does **not** provide latency histograms, HTTP error
rates, active session counts, or circuit-breaker state.

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

The Prometheus deployment uses one-hour retention and no persistent volume; it is
for local testing, not long-term evaluation data storage.

## Run a Simulated Call Lifecycle

The existing API and dashboard can exercise a call lifecycle without live
Twilio. Keep the API port-forward running. Use a unique `call_sid` each time:

```powershell
curl.exe --noproxy "*" -X POST http://127.0.0.1:18080/api/v1/calls/incoming `
  -H "content-type: application/json" `
  -d '{"call_sid":"CA-phase5-demo-001","from":"+6590000000","to":"+6560000000"}'
```

Record a turn and end the call:

```powershell
curl.exe --noproxy "*" -X POST http://127.0.0.1:18080/api/v1/calls/CA-phase5-demo-001/turn `
  -H "content-type: application/json" `
  -d '{"transcript":"I have been feeling overwhelmed."}'

curl.exe --noproxy "*" -X POST http://127.0.0.1:18080/api/v1/calls/CA-phase5-demo-001/hangup
```

The simulator stores active session state in Redis and archives a completed
session to PostgreSQL. The browser UI is available at
<http://127.0.0.1:18080/dashboard>.

## Planned k6 and Chaos Run

There is currently no checked-in k6 script, so the following benchmark is a
planned next step, not a command that works yet. Add a script (for example
`tests/k6/call-lifecycle.js`) that creates a unique call SID for each virtual
user, checks every response, records latency and errors, and executes incoming,
turn, and hangup in order. Then install k6 and run, for example:

```powershell
k6 run --vus 100 --duration 2m tests/k6/call-lifecycle.js
```

Use a short smoke run first, then increase concurrency. Record the script
revision, VUs, duration, request count, throughput, latency percentiles,
failure rate, API replica counts, and cluster conditions with each result.
Do not claim TEST-01 complete until a full lifecycle scenario is implemented
and its results are saved.

For TEST-02, run the k6 scenario against the test cluster and delete one API pod
while calls are active. First inspect the selected pod:

```powershell
kubectl -n sayangcare get pods -l app=sayangcare-api
```

In another terminal, select a pod name from that output and delete only that
pod:

```powershell
kubectl -n sayangcare delete pod API_POD_NAME
kubectl -n sayangcare rollout status deployment/sayangcare-api
```

Capture the k6 failures/latencies during deletion and verify the deployment
returns to 3/3 ready. A later successful request is not proof that an in-flight
call survived. Run destructive failover tests only against the disposable test
cluster, never a production database.

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
