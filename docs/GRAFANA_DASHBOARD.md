# SayangCare Grafana Dashboard

This guide describes the OBS-02 dashboard requirements, the checked-in
provisioning, and how to deploy and verify it on the local Minikube cluster.

**Status:** Grafana provisioning and all seven dashboard panels are checked in.
Run the deployment steps below to install or update Grafana in a local cluster.

## OBS-02 requirements

- Deploy Grafana in the `monitoring` namespace.
- Configure Grafana to query the Prometheus server already used by the project.
- Provision the dashboard from repository configuration so it can be recreated
  after Grafana is redeployed.
- Include panels for:
  - HTTP throughput.
  - HTTP error rate.
  - p95 and p99 HTTP response duration.
  - Active Redis sessions.
  - Session archive backlog.
  - Circuit breaker states.
  - Discovered API replicas and healthy Prometheus scrapes.
- Confirm Prometheus has API metrics and Grafana can query them.

The dashboard and Grafana settings are in
[`k8s/monitoring/grafana-values.yaml`](../k8s/monitoring/grafana-values.yaml).
The Prometheus and adapter configuration is in
[`k8s/monitoring/prometheus-values.yaml`](../k8s/monitoring/prometheus-values.yaml)
and
[`k8s/monitoring/prometheus-adapter-values.yaml`](../k8s/monitoring/prometheus-adapter-values.yaml).
The dashboard has UID `sayangcare-phase5` and is provisioned as read-only.

## Prerequisites

- Docker Desktop, Minikube, `kubectl`, and Helm installed.
- The Phase 4 Minikube profile, SayangCare API, PostgreSQL, and Redis running.
- Prometheus installed in the `monitoring` namespace and scraping the API pods.
- From the repository root, with PowerShell open.

For a fresh cluster, complete the one-time setup in
[`PHASE_4_KUBERNETES.md`](./PHASE_4_KUBERNETES.md) first.

## Deploy or update Grafana

If the Prometheus stack is not installed yet, follow the Prometheus and Adapter
deployment commands in [`PHASE_5_Windows.md`](./PHASE_5_Windows.md). Then add
the Grafana chart repository and create its admin secret if this is the first
install:

```powershell
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update

if (-not (kubectl --context minikube -n monitoring get secret grafana-admin --ignore-not-found -o name)) {
  $bytes = New-Object byte[] 32
  $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
  try { $rng.GetBytes($bytes) } finally { $rng.Dispose() }
  $password = [Convert]::ToBase64String($bytes)
  kubectl --context minikube -n monitoring create secret generic grafana-admin `
    --from-literal=admin-user=admin --from-literal="admin-password=$password"
}

helm upgrade --install grafana grafana/grafana `
  --namespace monitoring --create-namespace `
  --values k8s/monitoring/grafana-values.yaml
kubectl --context minikube -n monitoring rollout status deployment/grafana
```

The values file provisions the Prometheus data source and dashboard. It enables
anonymous **Viewer** access for local use, so opening the dashboard does not
require the Grafana admin password. The generated secret is retained in the
cluster for Grafana's admin account.

## One-command local startup

The repository also includes a Windows launcher for the full existing local
stack. After completing the one-time Phase 4 Minikube setup (including
installing the SayangCare application), run this from the repository root:

```powershell
.\scripts\start-observability.ps1 -OpenBrowser
```

It checks Docker, starts the existing Minikube profile if needed, waits for
PostgreSQL, Redis, Sentinel, and the API, then installs or upgrades Prometheus,
the Prometheus Adapter, and Grafana using the checked-in values files. It
creates the Grafana admin secret only if one does not already exist. The
launcher starts temporary local forwards for the Call Lab, Grafana, and
Prometheus; checks API readiness and metrics, Prometheus API targets, and
Grafana's Prometheus connection; then opens the Grafana dashboard. If a
preferred local port is occupied, it chooses a nearby free port and prints
the actual URLs.

Keep the terminal open while using the pages. Press Ctrl+C to stop the
temporary port-forwards; the cluster and installed services remain running.
The launcher does not create a new cluster, install the Phase 4 database
operator/application, rebuild the API image, or change existing database
credentials. Follow [Phase 4 setup](./PHASE_4_KUBERNETES.md) once for a fresh
Minikube profile. Helm needs internet access when it downloads or updates
charts.

Omit `-OpenBrowser` to keep the launcher from opening Grafana automatically.
To choose different starting ports:

```powershell
.\scripts\start-observability.ps1 -ApiPort 18080 -GrafanaPort 13000 -PrometheusPort 19090
```

If PowerShell blocks the local script, allow scripts in the current terminal
only:

```powershell
Set-ExecutionPolicy -Scope Process RemoteSigned
```

## Open the dashboard

Start a local port-forward and keep that PowerShell window open:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

Open:

```text
http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api
```

## Verify the panels

1. Confirm Grafana reports healthy:

   ```powershell
   kubectl --context minikube -n monitoring get pods
   ```

   Grafana, Prometheus, and the Prometheus Adapter pods should be `Running` or
   `Completed` as appropriate, and their ready containers should be ready.

2. Confirm the API targets are being scraped. In Prometheus, open
   `http://127.0.0.1:19090/targets` if its port-forward is running. The
   `sayangcare-api` targets should show `UP`. The dashboard's API replicas
   panel reports both discovered and healthy target counts.

3. Generate some safe local traffic using health endpoints (no phone call is
   needed). With the API forwarded to port 18080, run:

   ```powershell
   1..10 | ForEach-Object { Invoke-WebRequest http://127.0.0.1:18080/api/v1/health | Out-Null }
   ```

   Wait at least one Prometheus scrape interval, then refresh Grafana. Throughput
   should show activity; latency should appear once the histogram has samples.
   The error panel may remain at zero if no HTTP errors occurred, and active
   sessions/archive backlog may correctly be zero when there is no pending work.

4. In Prometheus or Grafana Explore, check the metrics directly:

   ```promql
   sum(rate(sayangcare_http_requests_by_route_total[5m]))
   ```

   ```promql
   histogram_quantile(0.95, sum by (le) (rate(sayangcare_http_request_duration_by_route_seconds_bucket[5m])))
   ```

   ```promql
   histogram_quantile(0.99, sum by (le) (rate(sayangcare_http_request_duration_by_route_seconds_bucket[5m])))
   ```

   ```promql
   max(sayangcare_redis_active_sessions)
   ```

   ```promql
   max(sayangcare_archive_backlog)
   ```

   ```promql
   max by (breaker) (sayangcare_circuit_breaker_state)
   ```

5. If a panel says **No data**, first check that its metric exists at
   `http://127.0.0.1:18080/api/v1/metrics`, then check the Prometheus target is
   `UP` and that the selected Grafana time range includes recent samples.
   Gauges can show zero when the system is healthy; an absent time series means
   the metric has not been exported or scraped.

The local Prometheus and Grafana configuration is intended for development and
demonstration. The checked-in values disable persistent storage, so Prometheus
history and local Grafana state can be lost when the cluster is recreated.
