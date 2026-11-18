# SayangCare

SayangCare is a distributed voice-support **prototype** for CSC3104 Cloud and
Distributed Computing. A Rust/Actix Web API maintains caller sessions, detects
synthetic high-risk turns, and coordinates volunteer handoffs. The dashboard
provides a simulated caller and volunteer workflow for the project demonstration.

> **Use synthetic data only.** This is not a clinical tool or emergency service.
> Dashboard role selectors are not authentication. Live Twilio calls, carrier
> audio and real caller outcomes have not been validated end to end.

## Contents

- [Architecture](#architecture)
- [Requirements](#requirements)
- [Windows setup](#windows-setup)
- [macOS setup](#macos-setup)
- [Dashboard demonstration](#dashboard-demonstration)
- [Configuration](#configuration)
- [Optional: three-node Raft](#optional-three-node-raft)
- [Kubernetes (Minikube)](#kubernetes-minikube)
- [Monitoring](#monitoring)
- [Testing and evaluation](#testing-and-evaluation)
- [API reference](#api-reference)
- [Troubleshooting](#troubleshooting)
- [Repository and submission evidence](#repository-and-submission-evidence)
- [Limitations](#limitations)

## Architecture

```text
 Dashboard / Twilio webhooks
              |
       Actix Web API  <---->  Groq (optional; breaker and fallback)
              |
       +------+---------------------------------------+
       |                      |                       |
  PostgreSQL             Redis / Sentinel     OpenRaft (optional)
  checkpoint authority   hot projection       3 fixed ownership voters
  transcripts + audit    risk DESC / FIFO     majority = 2 of 3
  generation fencing     waiting queue
```

The application is a **modular monolith**, not seven separately deployed
services. PostgreSQL commits versioned checkpoints before acknowledging session
mutations, archives terminal transcripts, and enforces ownership generations.
Redis supplies best-effort hot projections and queue order; Sentinel monitors
and promotes replicas in the Kubernetes deployment. Circuit breakers, bounded
AI concurrency and response deadlines protect against slow inference.

Optional OpenRaft orders volunteer ownership changes. An initial claim receives
generation 1, transfer 2, release 3 and reclaim 4. PostgreSQL rejects protected
actions with a stale generation (`409 Conflict`). PostgreSQL-only mode can
enforce fencing too. Kubernetes HPA API replicas are **not** Raft voters:
the default Kubernetes deployment uses PostgreSQL-only ownership.

## Requirements

| Requirement | Local dashboard | Kubernetes / evaluation |
| --- | --- | --- |
| Docker Desktop with running Linux container engine and Compose | Required | Required for Docker-driver Minikube |
| Git | To clone | To clone |
| Rust and Cargo 1.88+ | Required for native API; not required for Compose | Required for Cargo tests |
| `kubectl`, Minikube | No | Required |
| Helm | No | Optional Prometheus/Grafana installation |
| k6 | No | Optional load testing |
| PowerShell | Windows already includes it | Windows evaluation scripts |

Install Docker Desktop from [Docker](https://docs.docker.com/get-started/get-docker/)
and Rust from [rustup](https://rustup.rs/). On Windows, use Docker Desktop's
WSL 2 backend and Linux containers. On macOS, use Docker Desktop for your
processor architecture. Native Rust builds also need the platform's C toolchain:
Visual Studio C++ Build Tools on Windows, or Xcode Command Line Tools on macOS
(`xcode-select --install`). For Kubernetes, install
[Minikube](https://minikube.sigs.k8s.io/docs/start/),
[`kubectl`](https://kubernetes.io/docs/tasks/tools/) and optionally
[Helm](https://helm.sh/docs/intro/install/).

Run all repository commands from its root directory. Allow time for the first
Rust build and container downloads. Do not copy real credentials into source
control, screenshots, logs or chat.

## Windows setup

Open **PowerShell** in the project directory and start Docker Desktop. If you
need a new checkout:

```powershell
git clone https://github.com/pieter20037/CSC3104-SayangCare.git
Set-Location CSC3104-SayangCare
docker version
docker compose version
```

`docker version` should show both client and server. If using the native API,
also check `rustc --version` and `cargo --version`. Create a local configuration
**only if one does not already exist**:

```powershell
if (-not (Test-Path .env)) { Copy-Item .env.example .env }
```

For a synthetic demo, keep the optional Groq API key unset, including in an
existing `.env` and inherited shell environment. The included Twilio account
values are placeholders and are not usable for live calls. For a fresh local
database, the example password is development-only; if changing it, update
`POSTGRES_PASSWORD`, `DATABASE_URL` and `SAYANGCARE__POSTGRES__URL` consistently.
Existing PostgreSQL volumes retain their original password; changing `.env`
alone does not change it.

### Option A: API in Docker Compose

```powershell
docker compose up -d --build
docker compose ps
docker compose logs --tail 50 api
curl.exe --noproxy "*" -i http://127.0.0.1:8080/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:8080/api/v1/ready
Start-Process "http://127.0.0.1:8080/dashboard"
```

The first build can take several minutes. Both endpoints should return HTTP
200 once PostgreSQL, Redis and the API are ready. If they do not, check logs
and [Troubleshooting](#troubleshooting). Keep the containers running for the
demo. Stop them later with `docker compose down`; named volumes are retained.
Only do this if you own the Compose deployment; it stops the project's services.

### Option B: infrastructure in Docker, API with Cargo

Use this **instead of** the Compose API, not simultaneously on the same ports.
From the project directory:

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
cargo run -p sayangcare-api --locked
```

Leave that terminal running. In another PowerShell terminal:

```powershell
curl.exe --noproxy "*" -i http://127.0.0.1:8081/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:8081/api/v1/ready
Start-Process "http://127.0.0.1:8081/dashboard"
```

The native API reads `.env` automatically; host defaults are PostgreSQL 5434,
Redis 6379, Sentinel 26379 and API 8081. Press `Ctrl+C` to stop the API.
These Compose services have **one Sentinel**, configured with a quorum of two:
they are a convenient local dependency stack, not a working Sentinel HA
failover cluster. The Kubernetes deployment supplies three Sentinels.

## macOS setup

Open **Terminal** in the project directory and start Docker Desktop. If needed:

```bash
git clone https://github.com/pieter20037/CSC3104-SayangCare.git
cd CSC3104-SayangCare
open -a Docker
# Wait for the Docker engine to start.
docker version
docker compose version
test -f .env || cp .env.example .env
```

For the native API, also check `rustc --version` and `cargo --version`.
The [Windows configuration notes](#windows-setup) also apply: keep the optional
AI key unset for deterministic simulation, use only placeholder Twilio values,
and keep all three local PostgreSQL password settings consistent for a fresh
volume. Existing database volumes do not change password when `.env` changes.

### Option A: API in Docker Compose

```bash
docker compose up -d --build
docker compose ps
docker compose logs --tail 50 api
curl --noproxy '*' -i http://127.0.0.1:8080/api/v1/health
curl --noproxy '*' -i http://127.0.0.1:8080/api/v1/ready
open http://127.0.0.1:8080/dashboard
```

Wait for both checks to return HTTP 200. Stop later with
`docker compose down`; this preserves named volumes.
Only stop a Compose deployment you own.

### Option B: infrastructure in Docker, API with Cargo

```bash
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
cargo run -p sayangcare-api --locked
```

Leave this terminal running. In a second Terminal window, from the checkout:

```bash
curl --noproxy '*' -i http://127.0.0.1:8081/api/v1/health
curl --noproxy '*' -i http://127.0.0.1:8081/api/v1/ready
open http://127.0.0.1:8081/dashboard
```

Press `Ctrl+C` to stop the native API. The Compose Sentinel is a local
single-instance dependency, not an HA failover demonstration. The macOS
instructions are provided for teammates; the historical performance results
below were recorded on Windows, not benchmarked on a Mac.

## Dashboard demonstration

1. Select **Caller lab**, start a fresh synthetic session and record a turn.
   The Call SID, transcript, risk and version appear in the session view.
2. End the ordinary call and inspect its completed transcript. PostgreSQL is
   the durable authority; Redis is a recoverable projection.
3. Use **Simulate crisis call** for a fresh synthetic escalation. Risk 4-5
   triggers immediate human-review handling; this is a phrase heuristic,
   not a clinical diagnosis.
4. Open **Volunteer desk**, select an on-shift volunteer and **Claim** the
   intended case. Transfer it to another on-shift volunteer. The displayed
   assignment generation advances; release/reclaim advances it again.
5. In Raft mode, show the **Raft cluster** panel and ownership authority.
   For a reproducible stale-token `409` and current-token `200` example,
   see the preserved [presentation and demo guide](docs/PRESENTATION_AND_AI_HANDOFF.md).

The roster and role controls are prototype selectors, not authenticated
identities. Simulated sessions do not place a phone call. The professor-approved
dashboard scope does not validate inbound speech, public callbacks or outbound
ringing.

## Configuration

The API reads `config/default.toml` and `SAYANGCARE__...` environment
variables; the binary also loads `.env` for local development. Docker Compose
uses `.env` for interpolation and explicitly supplies its container variables;
the host's `127.0.0.1` database URL is **not** the container address.
Kubernetes uses a ConfigMap and Secrets. Never share `.env` or real credentials.

| Setting | Purpose |
| --- | --- |
| `POSTGRES_PASSWORD` | Local Compose database password; development only |
| `DATABASE_URL`, `SAYANGCARE__POSTGRES__URL` | Host SQLx/API PostgreSQL URL; normally port 5434 |
| `SAYANGCARE__SERVER__PORT` | Native API port; example 8081; container listens on 8080 |
| `SAYANGCARE__REDIS__SENTINEL_ENDPOINTS` | Sentinel endpoints appropriate to the deployment |
| `SAYANGCARE__REDIS__MASTER_URL` | Direct Redis URL for native local development |
| `SAYANGCARE__QUEUE__RAFT_ASSIGNMENTS` | Enable Raft ownership coordination when configured |
| `SAYANGCARE__AI__API_KEY` | Optional provider key; omit for deterministic fallback |
| `SAYANGCARE__PERFORMANCE__*` | Caller-response budget, timeouts and AI concurrency |
| `SAYANGCARE__TELEPHONY__*` | Twilio credentials, caller ID and public callback origin |

To use Groq with the native API, set the optional key privately in `.env`
and restart the API; provider calls may consume quota. Compose and Kubernetes
must explicitly receive optional keys in their respective configuration before
they can use them. Do not assume a key in the host `.env` is automatically
passed into the Compose API container.

The API applies the checked-in SQLx migrations during startup. Back up
valuable databases before upgrades; never edit a migration already applied.
`migrations/0010_session_checkpoints.sql` defines durable checkpointing and
`0011_assignment_fencing.sql` adds the ownership journal and fencing schema.
Legacy Raft assignments without generations may require explicit offline
reconciliation; do not delete data to bypass a refusal.

## Optional: three-node Raft

This is a separate local Compose deployment with fixed voters on ports
18081-18083. It builds images and starts containers; coordinate first on a
shared machine. With the same Compose project it shares PostgreSQL and Redis
with the single-API Compose setup, not the Kubernetes database. Avoid
simultaneous conflicting Compose deployments.

**Windows:**

```powershell
.\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
```

The launcher builds, starts and checks the Raft cluster. Alternatively, the
following Compose commands work in both PowerShell and macOS Terminal:

```text
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --build
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft ps
```

Open `http://127.0.0.1:18081/dashboard` after startup. Check the three
`/api/v1/queue/raft` endpoints at 18081, 18082 and 18083 for one leader and
two followers. Wait for readiness/election; investigate logs if it fails.
Status snapshots from followers are diagnostics, not a linearizable read API.
To stop this stack without deleting volumes:

```text
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft down
```

Ownership changes require a majority. Quorum loss fails new Raft-coordinated
claims/transfers closed; it does not automatically make every unrelated read
or note unavailable. A timeout may mean an unknown committed outcome: inspect
the ownership state and retry the **same request ID and payload**, rather than
issuing a conflicting replacement. Database and Raft are not one transaction;
the durable operation journal supports recovery. PostgreSQL-owned legacy cases
remain PostgreSQL-owned when Raft is enabled.

## Kubernetes (Minikube)

This is an **optional local test cluster**, not a production installation.
The deployment includes three API replicas at rest, three Redis nodes, three
Sentinels and a three-instance CloudNativePG cluster. All can still run on one
physical node; that does not prove node or zone high availability. Use a
dedicated cluster with sufficient Docker Desktop CPU, memory and disk.

### First-time cluster setup: Windows and macOS

Use a terminal in the repository root. Start Docker Desktop and verify the
selected Minikube context. These commands use no shell-specific syntax:

```text
minikube start --driver=docker
kubectl --context minikube get nodes
minikube addons enable storage-provisioner
minikube addons enable metrics-server
minikube addons enable ingress
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev --profile minikube
kubectl --context minikube apply -f k8s/base/namespace.yaml
```

The following Secret commands are **for a new disposable cluster only**. Check
that neither Secret already exists before running them; they deliberately use
test-only values and must not overwrite an existing deployment's credentials.
Use a proper secret-management process and strong credentials outside local
simulation. The two database passwords must match.

```text
kubectl --context minikube -n sayangcare get secret sayangcare-secrets sayangcare-db-app
```

If both are absent, create them:

```text
kubectl --context minikube -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD=local-dev-password --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID=ACtest --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN=test
kubectl --context minikube -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password=local-dev-password
```

If either Secret exists, inspect the existing deployment and coordinate with
its owner; do not replace credentials or expose Secret contents. Install the
pinned CloudNativePG operator if absent, then deploy:

```text
kubectl --context minikube apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl --context minikube -n cnpg-system rollout status deployment/cnpg-controller-manager --timeout=5m
kubectl --context minikube apply -k k8s/overlays/minikube
kubectl --context minikube -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl --context minikube -n sayangcare rollout status statefulset/redis --timeout=5m
kubectl --context minikube -n sayangcare rollout status statefulset/redis-sentinel --timeout=5m
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=5m
kubectl --context minikube -n sayangcare get pods,svc,pvc,hpa
```

These are first-time setup instructions, not a migration from an existing
PostgreSQL installation. Back up and verify existing data before upgrades or
changing deployments. Applying the overlay does not copy an old database.
Metrics Server may require a few minutes before `kubectl top nodes` works.

### Open an existing Kubernetes dashboard

**Windows PowerShell:**

```powershell
.\scripts\start-dashboard.ps1 -Mode Kubernetes -OpenBrowser
```

Keep the terminal open. The launcher checks an already installed application;
it does not perform the first-time setup. By default it opens port 18080.

**macOS Terminal:** leave this command running in one terminal:

```bash
kubectl --context minikube -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

In another terminal:

```bash
curl --noproxy '*' -i http://127.0.0.1:18080/api/v1/health
curl --noproxy '*' -i http://127.0.0.1:18080/api/v1/ready
open http://127.0.0.1:18080/dashboard
```

The Service port-forward selects one pod; if that pod is replaced it may
disconnect. Restart the forward. For failover load tests use the ingress
controller path, not this dashboard forward. To deploy source changes, rebuild
and load the image, restart the API Deployment, then check rollout status.
A source checkout by itself does not change a running container.

## Monitoring

The API uses the Rust `prometheus` client to export per-route/method/status
request counters and duration histograms, alongside Redis projection counts,
archive backlog and breaker states. It does **not** use `actix-web-prom`.
Prometheus and Grafana are optional for the basic dashboard.

For a dedicated Minikube cluster, install Helm and check existing releases
before modifying a shared monitoring namespace. Then:

```text
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
helm upgrade --install prometheus prometheus-community/prometheus --namespace monitoring --create-namespace --values k8s/monitoring/prometheus-values.yaml
helm upgrade --install prometheus-adapter prometheus-community/prometheus-adapter --namespace monitoring --values k8s/monitoring/prometheus-adapter-values.yaml
```

The Grafana chart expects an existing `grafana-admin` Secret. On a **new
disposable local cluster only**, create it with a test password, then install:

```text
kubectl --context minikube -n monitoring create secret generic grafana-admin --from-literal=admin-user=admin --from-literal=admin-password=local-demo-only
helm upgrade --install grafana grafana/grafana --namespace monitoring --values k8s/monitoring/grafana-values.yaml
kubectl --context minikube apply -k k8s/overlays/request-rate-hpa
kubectl --context minikube -n monitoring rollout status deployment/prometheus-server
kubectl --context minikube -n monitoring rollout status deployment/prometheus-adapter
kubectl --context minikube -n monitoring rollout status deployment/grafana
kubectl --context minikube get apiservice v1beta1.custom.metrics.k8s.io
```

Use a real secret-management process for anything beyond a disposable local
cluster. The request-rate overlay needs Prometheus Adapter; the base Minikube
overlay provides CPU/memory HPA. On Windows,
`.\scripts\start-observability.ps1 -OpenBrowser` can launch the existing
monitoring view. Otherwise, in separate terminals:

```text
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
```

Open `http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api` for the provisioned dashboard
and `http://127.0.0.1:19090` for Prometheus. Grafana's local anonymous access
is viewer-only; local dashboard/Prometheus storage is not persistent.
Redis key counts are projection counts, not authoritative session counts.

Example queries:

```promql
sum(rate(sayangcare_http_requests_by_route_total[5m]))
histogram_quantile(0.95, sum by (le) (rate(sayangcare_http_request_duration_by_route_seconds_bucket[5m])))
max(sayangcare_redis_active_sessions)
max(sayangcare_archive_backlog)
max by (breaker) (sayangcare_circuit_breaker_state)
```

## Testing and evaluation

### Rust workspace

In PowerShell or macOS Terminal:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
```

Run each command only after the previous one succeeds. SQLx offline metadata is
checked in, so normal compilation does not need a database. Tests marked
`ignored` are not passes; database integration tests need explicitly configured
`TEST_DATABASE_URL` and `TEST_REDIS_URL` pointing to **disposable** test
infrastructure. Never print those values or use a real patient database.

```text
cargo test -p sayangcare-session-store --locked transactional_fencing_and_ambiguous_commit_recovery -- --ignored
cargo test -p sayangcare-api --locked http_claim_race_transfer_notes_and_stale_owner_are_fenced -- --ignored
```

### Load, failover and fencing

Only run synthetic workloads on a dedicated, approved cluster. Fault tests can
interrupt shared services. Preserve prior reports and check native k6 exit
codes, failed lifecycle steps, acknowledged IDs, transcript contents and
database audits. `99` indicates failed k6 thresholds, not a pass.

The checked-in k6 scenario is `tests/k6/call-lifecycle.js`. On Windows, the
evaluation wrappers are:

```powershell
# These generate load; the last three inject failures. Review prompts first.
.\tests\k6\evaluate-lifecycle.ps1 -Mode baseline -Vus 100 -Duration 30s
.\tests\k6\evaluate-lifecycle.ps1 -Mode pod-failover -Vus 100 -Duration 60s
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres -Vus 20 -Duration 120s
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis -FaultMode master-pause -PauseMilliseconds 60000 -Vus 20 -Duration 120s
```

HPA evaluation requires a separately owned ingress-controller port-forward
and the complete cooldown:

```powershell
# Terminal 1; keep open
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18193:80
# Terminal 2
.\tests\k6\evaluate-hpa.ps1 -BaseUrl "http://127.0.0.1:18193" -Vus 100 -Duration 120s -DownscaleTimeoutSeconds 900
```

The comparison `.\scripts\test-raft-fencing.ps1` requires the Raft and
PostgreSQL-only endpoints described in the script. It creates fresh synthetic
cases; container fault injection is separately opt-in via `-InjectFaults`
and requires approval. Review restoration and NOT RUN/N/A outcomes.
See the [presentation and demo guide](docs/PRESENTATION_AND_AI_HANDOFF.md)
for exact evidence commands, workload boundaries and step-by-step examples.
Use a fresh `-OutputDirectory` for each run to keep historical evidence
separate; the scripts create timestamped files and record their own results.

The Windows wrappers use Windows-specific paths and executables; they are not
validated as macOS automation. On macOS, k6 can run the checked-in JavaScript
directly against an ingress forward, but a raw k6 run alone is **not** the
full Windows SQL/acknowledged-ID audit or failover acceptance test.

## API reference

| Route | Purpose |
| --- | --- |
| `GET /dashboard` | Synthetic Caller lab, Volunteer desk and Operations |
| `GET /api/v1/health` | Process liveness |
| `GET /api/v1/ready` | Redis and PostgreSQL readiness |
| `GET /api/v1/metrics` | Prometheus metrics for this API process |
| `POST /api/v1/calls/incoming` | Create/retry a session with stable Call SID |
| `GET /api/v1/calls/{sid}` | Inspect a session |
| `POST /api/v1/calls/{sid}/turn` | Record a caller turn |
| `POST /api/v1/calls/{sid}/hangup` | Complete/archive a call |
| `GET /api/v1/volunteers/queue` | Waiting cases ordered by risk/FIFO |
| `POST /api/v1/volunteers/claim` | Claim a case |
| `POST /api/v1/volunteers/{volunteer}/cases/{sid}/transfer` | Transfer ownership |
| `POST /api/v1/volunteers/{volunteer}/cases/{sid}/release` | Release to queue |
| `POST /api/v1/volunteers/{volunteer}/cases/{sid}/resolve` | Resolve a handoff |
| `POST /api/v1/volunteers/{volunteer}/cases/{sid}/notes` | Add a fenced note |
| `GET /api/v1/calls/{sid}/ownership[/audit]` | Current ownership and history |
| `GET /api/v1/queue/raft` | Raft diagnostics when enabled |
| `POST /twilio/voice`, `/twilio/gather`, `/twilio/status`, `/twilio/outbound-connect` | Signed Twilio webhooks; voice handlers return TwiML |

The embedded `crates/api/assets/holding-v1.wav` is a locally generated,
six-second chime served at `/audio/holding-v1.wav`. If replacing it, version
both its filename and URL to avoid stale cached audio.

Transfer/release/resolve require `assignment_generation` and `request_id`;
notes also require `body`. Transfer may include `to_volunteer_id`. Use a new
request ID for a new operation and retain it for retries. Stale generations
return 409; pending/unavailable coordination can return 503. See the demo
guide for complete request examples. Never interpret a timeout as proof
that an operation did not commit.

The Twilio integration verifies `X-Twilio-Signature`. A real deployment needs
valid credentials, a correctly configured public HTTPS callback URL, a
voice-capable number, carrier/regulatory compliance and explicit live tests.
The team tried a US trial number; that is not a claim that every trial provides
only US numbers. A funded Singapore pilot remains conditional on availability
and applicable requirements.

## Troubleshooting

| Symptom | Check |
| --- | --- |
| Docker engine unavailable | Start Docker Desktop and rerun `docker version`. |
| Port already in use | Choose a free port; do not terminate another person's service. |
| Native API cannot connect to PostgreSQL | Verify `.env`, port 5434, matching password and `docker compose ps`; existing volumes retain their old credentials. |
| API returns 503 on `/ready` | Check `docker compose logs` or Kubernetes pod logs; Redis and PostgreSQL must both be reachable. |
| Unexpected live inference | Remove the optional AI key from local `.env` and inherited environment, then restart; use synthetic sessions only. |
| Image or dashboard looks old | Rebuild/load the image and roll out the API; code changes do not update a running container. |
| Kubernetes cluster resource unknown | Install the CloudNativePG operator before applying the overlay. |
| HPA custom metric unavailable | Verify Prometheus Adapter and allow time for scraping; CPU/memory HPA works without the request-rate overlay. |
| Raft has no leader | Check all three nodes, peer configuration and ports; never silently switch a Raft-owned case to PostgreSQL. |
| Signature rejected | Verify exact public callback URL and Twilio token privately. Live calling is not required for the dashboard. |
| Cargo ignored tests | Configure disposable test dependencies and explicitly run them; do not count ignored tests as passes. |

Useful read-only checks:

```text
docker compose ps
docker compose logs --tail 100 api
kubectl --context minikube -n sayangcare get pods,svc,pvc,hpa
kubectl --context minikube -n sayangcare logs deployment/sayangcare-api
```

Avoid `docker compose down -v`, `minikube delete`, deleting PVCs or resetting
databases unless the data owner explicitly approves permanent data loss.

## Repository and submission evidence

| Path | Contents |
| --- | --- |
| `crates/` | API, core domain, session-store, queue, breaker, telephony and Redis crates |
| `config/`, `.env.example` | Non-secret defaults and local configuration template |
| `migrations/`, `.sqlx/` | PostgreSQL migrations and SQLx offline metadata |
| `docker-compose.yml`, `docker-compose.raft.yml`, `Dockerfile` | Local deployments |
| `k8s/` | Base, Minikube/HPA overlays and monitoring values |
| `tests/`, `scripts/` | Workspace, lifecycle, reliability and demonstration tooling |
| `artifacts/` | Timestamped historical evaluation reports and audits |
| [Report source](docs/GROUP25.md) | Preserved IEEE article source |
| [Final report PDF](documents/GROUP25_FINAL_REVISED.pdf) | Submitted-report candidate; review before submission |
| [Final report Word](documents/GROUP25_FINAL_REVISED.docx) | Editable report; review before submission |
| [Presentation and AI handoff](docs/PRESENTATION_AND_AI_HANDOFF.md) | Seven-person presentation, demo, statistics and evidence provenance |
| [Report metrics traceability](docs/REPORT_METRICS_TRACEABILITY.md) | Exact artifacts, JSON keys, chart columns and read-only Q&A proof commands |

Selected **historical** Windows experiments are indexed in
[`artifacts/phase6-current/final-evaluation.json`](artifacts/phase6-current/final-evaluation.json).
On the earlier four-CPU image, the 100-VU, 120-second HPA run recorded 9,248
lifecycles, zero HTTP failures and p95/p99 281.64/507.44 ms, with 3 -> 10 -> 3
API replicas. A separate later two-CPU PostgreSQL replacement run
([report](artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json))
recorded 2,216 lifecycles, zero HTTP failures and 7.16-second recovery.
Different image/cluster conditions prevent an isolated causal comparison.
Earlier failed runs remain evidence; later full-image HPA/API-failover
regression is still needed. Process-fault Raft evidence and the later
411-assertion harness are separate experiments, not one all-inclusive test.

## Limitations

- No verified end-to-end live telephony, production DNS/TLS or emergency use.
- Local one-node Minikube does not validate machine/zone failure or backup
  restoration; PostgreSQL outages can still cause explicit failures.
- No complete periodic Redis-projection reconciliation or automatic safe
  checkpoint/journal/tombstone retention policy.
- No transactional outbox or exactly-once guarantee for external phone effects.
- Heuristic risk scoring, prototype identity and transcript privacy controls
  require clinical, security and governance work before any real-world pilot.
- Benchmark results are bounded synthetic observations, not production
  service-level guarantees or proof of the latest combined image under all load.
