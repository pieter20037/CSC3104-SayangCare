# Phase 5: Chaos Testing, Metrics, and Final Evaluation (macOS)

For PowerShell commands, see the [Windows guide](./PHASE_5_Windows.md).

This guide records the Phase 5 backlog, what is already available, and how to
start a local Kubernetes stack and run the checks that exist today.
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
- k6 is needed for TEST-01/TEST-02, but a project k6 script must be written
  before `k6 run` can execute the call-lifecycle benchmark.

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
kubectl -n monitoring rollout status deployment/prometheus-server
kubectl -n monitoring rollout status deployment/prometheus-adapter
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

## Planned k6 and Chaos Run

**Stop here for now: the k6 benchmark is not implemented yet.**
The simulated-call commands above work independently of k6.

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

Installing k6 does not create the project's load-test script. There is currently
no checked-in `tests/k6/call-lifecycle.js`, so do not run that path yet.
The future script must create a unique call SID for each iteration, check every
response, record latency and errors, and execute incoming, turn, and hangup in
order. Its target URL must also be configured for the chosen test route.

Use a short smoke run first, then increase concurrency. Record the script
revision, VUs, duration, request count, throughput, latency percentiles,
failure rate, API replica counts, and cluster conditions with each result.
Do not claim TEST-01 complete until a full lifecycle scenario is implemented
and its results are saved.

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
