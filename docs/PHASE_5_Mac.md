# Phase 5: Chaos Testing, Metrics, and Final Evaluation (macOS)

For PowerShell commands, see the [Windows guide](./PHASE_5_Windows.md).

This guide records the Phase 5 backlog, how to start a local Kubernetes stack,
and how to run its checks. The repository now includes a k6 lifecycle scenario
and local Grafana dashboard. The results in
[PHASE_5_LOCAL_RESULTS.md](./PHASE_5_LOCAL_RESULTS.md) are from Windows and are
not macOS benchmark results. The API exposes HTTP latency/error and route
metrics along with active-session and circuit-breaker gauges.

## Milestones and Current Status

| ID | Milestone | Status | Current evidence / remaining work |
| --- | --- | --- | --- |
| **API-04** | Actix Web instrumentation | **Implemented** | `GET /api/v1/metrics` exposes HTTP status counters, per-route request counts/histograms, active-session/archive-backlog gauges, and breaker state. |
| **OBS-01** | Prometheus scraping | **Verified locally** | Local Prometheus scrapes API metrics and the Adapter publishes `sayangcare_http_requests_per_second`; active-session and breaker metrics are also exported. |
| **OBS-02** | Grafana dashboard | **Implemented locally** | `k8s/monitoring/grafana-values.yaml` provisions a local request-rate and counter dashboard. Windows deployment was verified; macOS has not been tested. |
| **TEST-01** | k6 concurrency benchmark | **Earlier HTTP-status run recorded; stronger checks ready to rerun** | The current scenario verifies response bodies and transcript retrieval after archival. Earlier Windows summaries predate those checks. |
| **TEST-02** | Pod failover chaos test | **Earlier pod-level run recorded; consistency checks ready to rerun** | Use the current archive consistency/retry metrics for a repeat. This is not node-level HA or a macOS test. |
| **TEST-03** | Evaluation metrics gathering | **Windows local evidence recorded** | Baseline, pod-loss, readiness, scaling, and PostgreSQL/Redis recovery results are in the local results report. A single-node cluster cannot establish node/zone HA. |
| **TEST-05** | HPA scaling performance evaluation | **Timed evaluation harness added; new results pending** | The Windows runner records replica timing and pairs HPA samples with k6 latency/error summaries; macOS-specific evaluation is not recorded. |

The local API HPA configuration includes CPU, memory, and request-rate metrics.
Its request-rate target is 25 requests/second per API pod.

## Requirements

- Docker Desktop for your Mac's architecture (Apple silicon or Intel), running
  Linux containers.
- Homebrew, Minikube, `kubectl`, and Helm. Install missing CLI tools with:

  ```bash
  brew install minikube kubectl helm
  ```

- The repository's Kubernetes manifests and an API image built for your cluster.
  An image loaded into another teammate's Minikube is not available on your Mac.
- A test cluster only for pod deletion and failover experiments. A single-node
  Minikube cluster cannot demonstrate node-level HA.
- k6 and the checked-in `tests/k6/call-lifecycle.js` script are used for
  TEST-01/TEST-02.

Use simulated caller data only. Do not use real patient/caller data in load
tests. Docker Desktop must be running, but do not also start Docker Compose or
a separate native API for this Kubernetes workflow.

## Start the Existing Local Kubernetes Stack

Open Terminal (zsh or bash), change to your repository folder, and start Docker
Desktop. Replace the example checkout path below with your own:

```bash
cd "$HOME/CSC3104-SayangCare"
open -a Docker
```

Wait until Docker Desktop reports that the engine is running. Then:

```bash
minikube start --driver=docker
kubectl config use-context minikube
kubectl get nodes
```

Wait for the CloudNativePG operator, database cluster, and application:

```bash
kubectl rollout status deployment/cnpg-controller-manager -n cnpg-system
kubectl -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel
kubectl -n sayangcare rollout status deployment/sayangcare-api
```

For a new Minikube profile, first create the namespace and test secrets, install
the CloudNativePG operator, build/load the API image, and deploy the application
as described in [Phase 4 Kubernetes setup](./PHASE_4_KUBERNETES.md). A new Mac
does not inherit the existing Windows cluster. Build locally so the image
matches your Mac's cluster architecture:

```bash
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev
```

Do not replace an existing database or re-create secrets without taking a
backup and following the Phase 4 guide's migration instructions.

Install the local Prometheus and adapter only if they are not already installed.
This is a one-time setup per cluster, not a step to repeat on every startup.
Check first:

```bash
kubectl -n monitoring get deployment prometheus-server prometheus-adapter
```

If both deployments exist, wait for readiness rather than reinstalling. If
they are missing, install:

```bash
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo update
helm upgrade --install prometheus prometheus-community/prometheus \
  --namespace monitoring --create-namespace \
  --values k8s/monitoring/prometheus-values.yaml
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter \
  --namespace monitoring \
  --values k8s/monitoring/prometheus-adapter-values.yaml
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
if ! kubectl -n monitoring get secret grafana-admin --ignore-not-found -o name | grep -q .; then
  password="$(openssl rand -base64 32)"
  kubectl -n monitoring create secret generic grafana-admin \
    --from-literal=admin-user=admin --from-literal="admin-password=$password"
fi
helm upgrade --install grafana grafana/grafana \
  --namespace monitoring \
  --values k8s/monitoring/grafana-values.yaml
kubectl -n monitoring rollout status deployment/prometheus-server
kubectl -n monitoring rollout status deployment/prometheus-adapter
kubectl -n monitoring rollout status deployment/grafana
```

Apply the request-rate overlay. It includes the Minikube HTTP Ingress settings
as well as the request-rate metric:

```bash
kubectl apply -k k8s/overlays/request-rate-hpa
kubectl get apiservice v1beta1.custom.metrics.k8s.io
kubectl -n sayangcare get pods,hpa
```

The overlay only needs reapplying when deploying or updating configuration.
For an unchanged existing cluster, the last two commands are enough to inspect
its state.

The Prometheus Adapter APIService should report `Available=True`. The custom
metric is available after Prometheus has scraped API traffic; initially it may
be absent until requests have been made and samples collected.
Grafana's chart emits a deprecation warning and this local dashboard uses no
persistent storage; do not treat it as a production installation.

To view the provisioned dashboard, forward Grafana in another Terminal:

```bash
kubectl -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

Open <http://127.0.0.1:13000>. Anonymous access is read-only and only exposed
through the localhost port-forward.

## Run the API and Inspect Metrics

In one Terminal window, forward the API service:

```bash
kubectl -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

In another terminal, check health, readiness, and the current metrics endpoint:

```bash
curl --noproxy "*" -i http://127.0.0.1:18080/api/v1/health
curl --noproxy "*" -i http://127.0.0.1:18080/api/v1/ready
curl --noproxy "*" http://127.0.0.1:18080/api/v1/metrics
```

Expected: health and readiness return HTTP 200, readiness reports Redis and
PostgreSQL as `ok`, and metrics include `sayangcare_http_requests_total`.
This endpoint currently does **not** provide latency histograms, HTTP error
rates, active session counts, or circuit-breaker state.

To inspect Prometheus, use a separate terminal:

```bash
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

```bash
kubectl get --raw "/apis/custom.metrics.k8s.io/v1beta1/namespaces/sayangcare/pods/*/sayangcare_http_requests_per_second"
kubectl -n sayangcare describe hpa sayangcare-api
```

The Prometheus deployment uses one-hour retention and no persistent volume; it is
for local testing, not long-term evaluation data storage.

## Run a Simulated Call Lifecycle

Keep the API port-forward running. Run these steps in the same zsh or bash
terminal. No live Twilio credentials are needed.

**1. Set up a unique test call:**

```bash
api="http://127.0.0.1:18080/api/v1/calls"
sid="SIM-$(uuidgen)"
```

**2. Start the call:**

```bash
body=$(printf '{"call_sid":"%s","from":"+6590000000","to":"+6560000000"}' "$sid")
curl --noproxy "*" -sS --fail-with-body "$api/incoming" -H "Content-Type: application/json" -d "$body"
```

**3. Record a turn:**

```bash
body='{"transcript":"I have been feeling overwhelmed."}'
curl --noproxy "*" -sS --fail-with-body "$api/$sid/turn" -H "Content-Type: application/json" -d "$body"
```

**4. End the call:**

```bash
curl --noproxy "*" -sS --fail-with-body -X POST "$api/$sid/hangup"
```

These commands use shell quoting supported by zsh and bash. If your Mac's
older curl does not support `--fail-with-body`, use `--fail` instead; failures
will still return a nonzero exit code, but their response bodies may be hidden.
If a step fails, inspect its error before proceeding.

The simulator stores active session state in Redis and archives a completed
session to PostgreSQL. The browser UI is available at
<http://127.0.0.1:18080/dashboard>.

## Existing Automated Regression Tests

With the Rust toolchain installed, run from the repository root:

```bash
cargo test --workspace --locked
```

These existing tests cover session state, risk/sentiment, archive retry and
recovery, circuit breakers/fallback, Twilio handling, telemetry, and Raft.
Most do not require Compose or Minikube. The 22 October 2026 Windows validation
passed 72 tests; the 2 Redis-dependent tests passed separately against disposable
Redis. This is not a macOS execution result or a k6/live-telephony validation.

For the ignored tests, provide isolated test Redis at `127.0.0.1:6379`, never
application/production Redis, then run:

```bash
cargo test --locked -p sayangcare-circuit-breaker -p sayangcare-priority-queue -- --ignored
```

## Run k6 and Chaos Checks

The checked-in k6 scenario creates unique synthetic calls, checks the incoming,
turn, and hangup responses, and pauses each VU for one second between calls.
For load tests, forward the Ingress controller so traffic can reach multiple
API replicas; forwarding directly to the API Service selects only one pod:

```bash
kubectl -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18081:80
```

If k6 is not installed, install it using Homebrew:

```bash
brew install k6
k6 version
```

If `k6` is not found after installation, check Homebrew's executable directly:

```bash
"$(brew --prefix)/bin/k6" version
```

If this prints a version, k6 is installed; check your Homebrew PATH setup rather
than reinstalling. Homebrew typically uses `/opt/homebrew` on Apple silicon and
`/usr/local` on Intel Macs. Follow `brew shellenv` instructions for your
installation, then restart Terminal or VS Code.

Run a short smoke test before increasing concurrency:

```bash
k6 run -e BASE_URL=http://127.0.0.1:18081 -e VUS=2 -e DURATION=10s tests/k6/call-lifecycle.js
```

For a 100-VU run with a saved JSON summary:

```bash
k6 run -e BASE_URL=http://127.0.0.1:18081 -e VUS=100 -e DURATION=30s \
  --summary-export docs/phase5-local-baseline.json tests/k6/call-lifecycle.js
```

The summary includes p50 (median), p95, p99, request rate, completed
lifecycles, and errors. Record the script revision, VUs, duration, API image,
replica counts, and cluster conditions with the result.

For TEST-02, run the k6 scenario against the test cluster and delete one API pod
while calls are active. First inspect the selected pod:

```bash
kubectl -n sayangcare get pods -l app=sayangcare-api
```

In another terminal, select a pod name from that output and delete only that
pod:

```bash
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

```bash
minikube stop
```
