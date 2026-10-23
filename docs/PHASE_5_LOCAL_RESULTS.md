# Local Phase 4 and 5 Results

**Run date:** 2026-10-23  
**Scope:** Local, single-node Minikube test cluster; generated synthetic callers
only. These results are not production or multi-node availability evidence.

## Test environment

| Component | Version / configuration |
| --- | --- |
| Docker Desktop engine | 29.8.1 |
| Minikube | 1.39.0, Docker driver |
| Kubernetes | 1.37.0, one node |
| Minikube resources | 6 CPUs, 8 GiB memory |
| API image | `sayangcare-api:dev`, `sha256:b6c223c9f08ace7307c6d77236086171ed80e866a43caee47861fd7410f9fa39` |
| Helm | 4.3.1 |
| Prometheus Adapter | APIService available; custom request-rate metrics returned for API pods |
| Grafana | 12.3.1, local ephemeral dashboard |
| k6 | 2.2.0 |

The API, Redis, Redis Sentinel, and PostgreSQL services were ready. The
request-rate HPA started with three API replicas and scaled up to five during
the load checks. The baseline and chaos tests used the local Ingress
port-forward, 100 VUs, 30 seconds, a one-second think time per VU, and unique
synthetic call IDs. Each iteration performed incoming, turn, and hangup.

## k6 results

| Test | Completed lifecycles | HTTP requests | Throughput (requests/s) | p50 HTTP latency | p95 | p99 | HTTP failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline, 100 VUs | 2,898 | 8,694 | 280.19 | 11.60 ms | 69.95 ms | 131.70 ms | 0 (0%) |
| API pod deleted during load, 100 VUs | 2,881 | 8,643 | 278.26 | 12.20 ms | 69.51 ms | 124.01 ms | 0 (0%) |

All lifecycle checks passed in both runs. The API pod replacement took about
7.7 seconds during the chaos run. The baseline and chaos k6 summaries are
preserved in [phase5-local-baseline.json](./phase5-local-baseline.json) and
[phase5-local-api-failover.json](./phase5-local-api-failover.json).

These numbers describe this local single-node test setup only. Completed call
lifecycles were measured separately from HTTP requests; HTTP request throughput
must not be reported as call throughput.

## Phase 4 recovery checks

- **API pod recovery:** Deleted one SayangCare API pod during the 100-VU run.
  The Deployment replaced it in approximately 7.7 seconds. API health and
  readiness remained HTTP 200; readiness reported PostgreSQL and Redis `ok`.
- **PostgreSQL primary failover:** Deleted primary `sayangcare-db-1`.
  CloudNativePG promoted `sayangcare-db-2` and restored three ready instances
  in approximately 62.7 seconds. A completed synthetic session and its two
  transcript turns were present on both the old and promoted primary.
- **Redis Sentinel failover:** Deleted the Redis master `redis-0` after Redis
  confirmed replication to two replicas. Sentinel promoted `redis-2`; the
  removed pod recovered in approximately 7.6 seconds. The active synthetic
  session remained readable after promotion and API readiness returned healthy.
- **Monitoring and dashboard:** Prometheus returned SayangCare request-counter
  series, the custom metrics API returned per-pod request-rate values, the HPA
  consumed that metric, and the provisioned three-panel Grafana dashboard and
  Prometheus datasource both loaded successfully.

## Limitations

- Minikube has one Kubernetes node. Pod replacement does not test node, zone, or
  external-site failure.
- API metrics currently expose only the process-local HTTP request counter.
  HTTP latency/error histograms, active-session metrics, and circuit-breaker
  metrics are not implemented.
- Prometheus retention is one hour and is not persisted. Grafana is configured
  without persistent storage for local validation.
- The checked-in Grafana chart is deprecated upstream; this local setup is not
  a production deployment recommendation.
- The tests use simulated API calls only; they do not validate Twilio,
  microphone audio, live inference-provider credentials, or public DNS/TLS.
- The load test is a 30-second local benchmark, not a long-duration capacity
  qualification.
