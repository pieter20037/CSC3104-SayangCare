# CSC3104-SayangCare

SayangCare is a Rust-based, voice-first telehealth service for handling inbound calls, maintaining conversation state, detecting distress, and escalating high-risk sessions to human volunteers.

This repository is the CSC3104 cloud-computing project. It demonstrates a modular backend with:

- Actix Web HTTP and Twilio webhook endpoints
- Redis Sentinel for hot, replicated session state
- PostgreSQL for durable session archives and audit data
- A Redis-backed priority queue for volunteer escalation
- An adaptive circuit breaker for degraded-mode behavior
- Docker Compose for local infrastructure
- Kubernetes manifests for a multi-replica deployment

The current codebase is a working scaffold. The JSON Call Lab simulator and live Twilio `/gather` path apply deterministic risk triage and can generate supportive replies through Groq's OpenAI-compatible Chat Completions API. High-risk turns bypass the LLM and use a fixed safety response before queue escalation. Without a Groq API key, both paths use deterministic fallback replies. Readiness checks Redis and PostgreSQL; `/api/v1/metrics` exports a per-process HTTP request counter. See [Known Limitations](#known-limitations).

## Contents

- [Architecture](#architecture)
- [Repository Layout](#repository-layout)
- [Requirements](#requirements)
- [Quick Start: Local Development](#quick-start-local-development)
- [Verification](#verification)
- [Useful Commands](#useful-commands)
- [Configuration](#configuration)
- [Database and Migrations](#database-and-migrations)
- [HTTP API](#http-api)
- [Twilio Webhook Flow](#twilio-webhook-flow)
- [Storage and Reliability Design](#storage-and-reliability-design)
- [Docker](#docker)
- [Kubernetes](#kubernetes)
- [Observability](#observability)
- [Troubleshooting](#troubleshooting)
- [Project Documentation](docs/README.md)
- [Phase 1 Test Guide](docs/PHASE_1_TEST_GUIDE.md)
- [Raft Leader Election Demo](docs/RAFT_DEMO_GUIDE.md)
- [Known Limitations](#known-limitations)
- [Development Guidelines](#development-guidelines)

## Architecture

```text
Caller
	|
	v
Twilio ------------------------+
	|                            |
	| form-encoded webhooks      | Twilio REST API
	v                            v
SayangCare API (Actix Web) --> Twilio
	|
	+--> Redis Sentinel --> Redis master/replicas
	|       |
	|       +--> hot session state, CAS updates, escalation queue
	|
	+--> PostgreSQL --> durable terminal sessions and transcripts
	|
	+--> adaptive circuit breaker --> degraded mode / escalation behavior
```

The API is built as a Cargo workspace. The `core` crate owns domain types and ports; infrastructure crates implement those ports; the `api` crate assembles everything and exposes HTTP routes.

## Repository Layout

| Path                      | Responsibility                                                         |
| ------------------------- | ---------------------------------------------------------------------- |
| `Cargo.toml`              | Workspace members, shared dependencies, Rust version, release profile  |
| `crates/core`             | Domain models, configuration, errors, and hexagonal-architecture ports |
| `crates/api`              | Actix Web binary, application bootstrap, routes, middleware, telemetry |
| `crates/session-store`    | Redis hot store, PostgreSQL archive store, and tiered storage facade   |
| `crates/priority-queue`   | Redis sorted-set queue and atomic volunteer claims                     |
| `crates/circuit-breaker`  | Circuit states, rolling counters, adaptive thresholds, registry        |
| `crates/telephony`        | Twilio REST API implementation of the telephony port                   |
| `crates/redis-client`     | Shared Redis connection provider with periodic Sentinel master refresh |
| `migrations/`             | SQLx PostgreSQL migrations                                             |
| `config/default.toml`     | Default non-secret application configuration                           |
| `docker-compose.yml`      | Local PostgreSQL, Redis, Redis Sentinel, and single API service        |
| `docker-compose.raft.yml` | Three-node Raft demo stack using the shared local infrastructure       |
| `Dockerfile`              | Multi-stage release image build                                        |
| `k8s/`                    | Kubernetes namespace, configuration, workloads, probes, and HPA        |
| `.sqlx/`                  | Committed SQLx offline query metadata                                  |

## Requirements

- Rust `1.88` or newer, including Cargo
- Docker Desktop with Docker Compose
- SQLx CLI only if applying or inspecting migrations manually
- `psql` is optional; it is available inside the PostgreSQL container
- `curl` for endpoint checks
- `kubectl` only if deploying to Kubernetes

Install SQLx CLI if needed:

```bash
cargo install sqlx-cli --no-default-features --features postgres,rustls
```

Check the toolchain:

```bash
rustc --version
cargo --version
docker --version
docker compose version
```

## Quick Start: Local Development

The native development workflow is the same on Windows and macOS: Docker runs PostgreSQL and Redis/Sentinel, while Cargo runs the API on port `8081`. The `.env.example` defaults use PostgreSQL `127.0.0.1:5434`, Redis `127.0.0.1:6379`, and Sentinel `127.0.0.1:26379`. Port `5434` avoids conflicts with host PostgreSQL services on `5432` or `5433`.

The repository ignores `.env` because it can contain credentials. Copy the example once and keep real credentials out of Git. The example Twilio values are placeholders; they let the local scaffold start but do not connect a real Twilio account.

### Windows (PowerShell)

From the repository root:

```powershell
if (-not (Test-Path .env)) { Copy-Item .env.example .env }
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

In terminal 1, start the API and leave it running:

```powershell
cargo run -p sayangcare-api
```

In terminal 2, check that it responds:

```powershell
curl.exe -i http://localhost:8081/api/v1/health
curl.exe -i http://localhost:8081/api/v1/ready
```

Both should return HTTP `200`. Use `curl.exe` in PowerShell to avoid the `curl` alias. The API stays in the foreground; do not start a second copy on port 8081. Press `Ctrl+C` in terminal 1 to stop it.

### macOS (Terminal)

From the repository root:

```bash
test -f .env || cp .env.example .env
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

In terminal 1, start the API and leave it running:

```bash
cargo run -p sayangcare-api
```

In terminal 2, check the endpoints:

```bash
curl -i http://localhost:8081/api/v1/health
curl -i http://localhost:8081/api/v1/ready
```

Both should return HTTP `200`. Press `Ctrl+C` in terminal 1 to stop the API.

The API applies embedded SQLx migrations at startup. To inspect migration status manually, install SQLx CLI and run `sqlx migrate info --source migrations` after setting `DATABASE_URL` from `.env`. See [Database and Migrations](#database-and-migrations).

Stop local infrastructure when finished; named database volumes are retained:

```bash
docker compose down
```

`docker compose down -v` also deletes the PostgreSQL data volume and is destructive.

## Verification

Run these three checks from the repository root for a fast Rust-code verification pass.

### Windows (PowerShell)

Each step stops the sequence if it fails:

```powershell
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { throw 'Formatting check failed' }
cargo check --workspace --all-targets
if ($LASTEXITCODE -ne 0) { throw 'Workspace compile check failed' }
cargo test --workspace
if ($LASTEXITCODE -ne 0) { throw 'Workspace tests failed' }
```

### macOS (Terminal)

The `&&` operators stop the sequence at the first failure:

```bash
cargo fmt --all -- --check && \
cargo check --workspace --all-targets && \
cargo test --workspace
```

`cargo fmt --check` checks formatting, `cargo check --workspace --all-targets` compiles the workspace and test targets, and `cargo test --workspace` runs automated tests. These commands do not prove Docker services or real Twilio connectivity work. For live local service checks, follow Quick Start and use the smoke-test flow in [PHASE_1_TEST_GUIDE.md](docs/PHASE_1_TEST_GUIDE.md). In the verified local smoke test, the same CallSid reused successfully, a conflicting caller returned `409`, a high-risk transcript escalated the session, and the Redis hot key was removed after the archive check.

## Useful Commands

### Rust development

```bash
# Format all Rust code
cargo fmt --all

# Check the complete workspace, including test targets
cargo check --workspace --all-targets

# Run all workspace tests
cargo test --workspace

# Run tests for one crate
cargo test -p sayangcare-session-store

# Build the release API binary
cargo build --release --bin sayangcare

# Run the API binary directly after building
cargo run --bin sayangcare

# View dependency tree
cargo tree

# Remove generated Rust build artifacts
cargo clean
```

SQLx query metadata is committed in `.sqlx/`, and `.cargo/config.toml` enables SQLx offline mode by default. This means normal compilation does not require a live PostgreSQL server.

### Docker operations

```bash
# Build and start the complete Compose stack
docker compose up -d --build

# Start only infrastructure for local Cargo development
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel

# Follow API logs
docker compose logs -f api

# Follow database logs
docker compose logs -f postgres

# Inspect running containers and resource usage
docker compose ps
docker stats --no-stream

# Rebuild the API image without using stale layers
docker compose build --no-cache api

# Stop services but preserve named volumes
docker compose down
```

### Database inspection

```bash
# Connect to the Docker PostgreSQL instance on the host
set -a
. ./.env
set +a
psql "$DATABASE_URL"

# List tables from inside the container
docker compose exec postgres psql -U sayangcare -d sayangcare -c '\dt'

# Inspect SQLx migration history
docker compose exec postgres psql -U sayangcare -d sayangcare \
	-c 'select version, description, success, installed_on from _sqlx_migrations order by version;'

# Verify an archived call after using End call in the dashboard
docker compose exec -T postgres psql -U sayangcare -d sayangcare \
	-c "SELECT id, state, handoff_status, assigned_volunteer_id, is_simulated, risk->'level' AS risk_level, jsonb_array_length(transcript->'turns') AS turns FROM sessions WHERE id = 'SIM-your-session-id';"

# Check the migration CLI state
unset DATABASE_URL SQLX_DATABASE_URL
sqlx migrate info --source migrations
```

## Configuration

Configuration is loaded in this order:

1. Optional `config/default.toml`
2. Environment variables with the `SAYANGCARE__` prefix
3. `.env` is loaded by the API binary for local development

For local development, keep the PostgreSQL credentials and URL together in `.env`; the API requires the database URL and Twilio credentials from environment variables. Docker Compose builds its internal URL from `POSTGRES_PASSWORD`. Create the Kubernetes Secret from `.env`; workloads map only the required credential keys from that Secret.

The double underscore maps environment variables to nested TOML sections. For example:

```text
SAYANGCARE__POSTGRES__MAX_CONNECTIONS=20
```

maps to:

```toml
[postgres]
max_connections = 20
```

Important settings:

| Setting                                             | Local example                          | Purpose                                                                                          |
| --------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------------------------------------ |
| `SAYANGCARE__SERVER__HOST`                          | `0.0.0.0`                              | HTTP bind address                                                                                |
| `SAYANGCARE__SERVER__PORT`                          | `8081`                                 | HTTP port                                                                                        |
| `SAYANGCARE__SERVER__WORKERS`                       | `4`                                    | Actix worker count                                                                               |
| `SAYANGCARE__POSTGRES__URL`                         | `...@127.0.0.1:5434/...`               | PostgreSQL connection URL                                                                        |
| `SAYANGCARE__POSTGRES__MAX_CONNECTIONS`             | `20`                                   | PostgreSQL pool limit                                                                            |
| `SAYANGCARE__REDIS__SENTINEL_ENDPOINTS`             | `["127.0.0.1:26379"]`                  | Sentinel endpoint list                                                                           |
| `SAYANGCARE__REDIS__MASTER_NAME`                    | `sayangcare-master`                    | Sentinel master name                                                                             |
| `SAYANGCARE__REDIS__MASTER_URL`                     | `redis://127.0.0.1:6379/`              | Optional direct session-store URL; set for native local development                              |
| `SAYANGCARE__REDIS__QUEUE_URL`                      | `redis://127.0.0.1:6379/`              | Redis endpoint used by the priority queue                                                        |
| `SAYANGCARE__REDIS__PASSWORD`                       | unset locally                          | Redis password, if enabled                                                                       |
| `SAYANGCARE__CIRCUIT_BREAKER__BASE_ERROR_RATE`      | `0.25`                                 | Base error threshold                                                                             |
| `SAYANGCARE__CIRCUIT_BREAKER__BASE_LATENCY_MS`      | `2000`                                 | Base latency threshold                                                                           |
| `SAYANGCARE__CIRCUIT_BREAKER__MIN_REQUESTS`         | `20`                                   | Minimum shared samples for error-rate and p95 evaluation; three consecutive failures trip sooner |
| `SAYANGCARE__CIRCUIT_BREAKER__HALF_OPEN_AFTER_SECS` | `10`                                   | Open-to-half-open delay                                                                          |
| `SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID`         | test value locally                     | Twilio account identifier                                                                        |
| `SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN`          | test value locally                     | Twilio credential                                                                                |
| `SAYANGCARE__TELEPHONY__TWILIO_FROM_NUMBER`         | unset                                  | Twilio-owned or verified caller ID for outbound volunteer calls                                  |
| `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL`            | `http://localhost:8081`                | Exact public origin used in TwiML actions and signature validation                               |
| `SAYANGCARE__AI__API_KEY`                           | unset                                  | Optional Groq API key; keep it in ignored `.env`                                                 |
| `SAYANGCARE__AI__MODEL`                             | `openai/gpt-oss-120b`                  | Groq-hosted chat model                                                                           |
| `SAYANGCARE__AI__BASE_URL`                          | Groq Chat Completions URL              | OpenAI-compatible chat completions endpoint                                                      |
| `SAYANGCARE__AI__TIMEOUT_SECS`                      | `12`                                   | Inference request timeout                                                                        |
| `RUST_LOG`                                          | `info,sayangcare=debug,actix_web=info` | Log filter                                                                                       |

Never commit real Twilio, LLM, database, or Redis credentials. Use Kubernetes Secrets or an external secret manager for deployed environments.

To enable Groq locally, put your Groq key in the ignored `.env` file as `SAYANGCARE__AI__API_KEY=...`, set the model and base URL shown in `.env.example`, then restart the API. Do not paste the key into source files, README, or chat. If the key is absent or a request fails, the API uses a deterministic fallback. The model generates supportive non-crisis replies; local heuristic triage remains responsible for immediate escalation.

## Database and Migrations

The initial migration in `migrations/0001_init.sql` creates:

- `sessions`: durable session state and transcripts
- `volunteers`: volunteer registry
- `escalations`: auditable session-to-volunteer assignments
- Indexes for caller, creation time, state, and open escalations

Later migrations: `0002` session recovery fields, `0003` volunteer handoff fields, `0004` simulated-session flag, `0005` volunteer assignments (one active owner per session), `0006` assignment history and reclaims, `0007` allow hot-session assignments. Never edit an applied migration; add a new one.

The API also runs embedded migrations during startup with `sqlx::migrate!`, so a running API expects the database to be reachable and migration-compatible.

### Important local port distinction

| Context                                     | PostgreSQL address                           |
| ------------------------------------------- | -------------------------------------------- |
| Host shell, `sqlx`, `psql`, local Cargo API | `127.0.0.1:5434`                             |
| API container in Compose                    | `postgres:5432`                              |
| Kubernetes API pod                          | `sayangcare-db-rw.sayangcare.svc.cluster.local:5432` (current primary) |

If `sqlx migrate run` reports `role "sayangcare" does not exist`, it is usually connecting to another PostgreSQL server on `localhost:5432`. Check the effective environment:

```bash
printf 'DATABASE_URL=%s\n' "${DATABASE_URL-<unset>}"
printf 'SQLX_DATABASE_URL=%s\n' "${SQLX_DATABASE_URL-<unset>}"
```

Then clear stale overrides and retry:

```bash
unset DATABASE_URL SQLX_DATABASE_URL
sqlx migrate run --source migrations
```

Or select the Docker database explicitly:

```bash
set -a
. ./.env
set +a
sqlx migrate run --source migrations
```

## HTTP API

All JSON application routes are under `/api/v1`.

### Health and readiness

```text
GET /api/v1/health
GET /api/v1/ready
GET /api/v1/metrics
```

Example:

```bash
curl -i http://localhost:8081/api/v1/health
```

`/health` returns service status and package version. `/ready` checks Redis and PostgreSQL concurrently with a two-second timeout per dependency and returns HTTP `503` if either check fails. `/metrics` exports the Prometheus counter `sayangcare_http_requests_total`. The counter is process-local; Prometheus should scrape each API pod separately and calculate each pod's rate.

### Call session routes

```text
POST /api/v1/calls/incoming
GET  /api/v1/calls/{session_id}
POST /api/v1/calls/{session_id}/turn
POST /api/v1/calls/{session_id}/hangup
```

For the browser-based Call Lab, start the API and open `http://localhost:<api-port>/dashboard`. Use the port shown in the API startup log; it depends on the effective configuration (the native-development examples in this README use `8081`, while Compose publishes `8080`).

### Call Lab simulator

The SID refresh control generates a new `SIM-...` ID. **Start call** creates a session with that ID in Redis; **Record turn** stores the caller and assistant turns there. The ID stays the same for the whole call. **End call** archives the session to PostgreSQL and removes its Redis copy only after the archive succeeds. If the call was escalated, its final state remains `escalated` when archived. With a Groq key configured, non-crisis turns receive a generated supportive reply; high-risk turns use the deterministic safety response and local heuristic classification. Without a key, turns use deterministic fallback replies. These are test transcripts, not microphone or phone audio.

Select an on-shift volunteer in **Working as** (Aisha is selected by default; Noor is also on shift; Ibrahim is off shift). A queue row's **Claim** action assigns that specific session to the selected volunteer. **Claim next** assigns the highest-priority pending session to that volunteer. The assignment appears under **Assigned cases** and in the session record. **View case details** loads the session and transcript. **Transfer** moves an assigned case to another on-shift volunteer (selected automatically); **Resolve** closes the volunteer handoff; **Acknowledge** records operator review. Simulated calls update SayangCare state only and never redirect a real phone call.

The dashboard is a responsive operator/test workspace with the call form, pending queue, session transcript, roster, assignments, alerts, and request details. If several local API copies are running, open the URL for the copy built from the latest source and confirm its port in that process's startup log.

Create a session:

```bash
curl -X POST http://localhost:8081/api/v1/calls/incoming \
	-H 'content-type: application/json' \
	-d '{"call_sid":"CA-demo-001","from":"+6590000000","to":"+6560000000"}'
```

The supplied `call_sid` is the session ID. Creation is atomic in Redis, so a retried request for the same CallSid and caller returns the existing session instead of resetting it; reuse of that CallSid by a different caller returns `409 Conflict`.

Record a caller turn:

```bash
curl -X POST http://localhost:8081/api/v1/calls/CA-demo-001/turn \
	-H 'content-type: application/json' \
	-d '{"transcript":"I have been feeling overwhelmed."}'
```

Complete a call:

```bash
curl -X POST http://localhost:8081/api/v1/calls/CA-demo-001/hangup
```

Turn batches increment the session version once and persist through Redis Lua compare-and-swap. Concurrent updates with a stale version return `409 Conflict`. Ending a call uses CAS before archiving the full session to PostgreSQL; Redis state is deleted only after the durable archive succeeds. An escalated call retains its `escalated` state when ended. On a cache miss, archived sessions are restored to Redis only if no concurrent request has already recreated them.

### Volunteer queue

```text
GET  /api/v1/volunteers/queue
POST /api/v1/volunteers/claim
GET  /api/v1/volunteers
GET  /api/v1/volunteers/{volunteer_id}/cases
POST /api/v1/volunteers/{volunteer_id}/cases/{session_id}/transfer
POST /api/v1/volunteers/{volunteer_id}/cases/{session_id}/resolve
```

Claim the highest-priority queued session for a volunteer:

```bash
curl -X POST http://localhost:8081/api/v1/volunteers/claim \
	-H 'content-type: application/json' \
	-d '{"volunteer_id":"volunteer-aisha"}'
```

To claim one particular pending session, include its ID:

```bash
curl -X POST http://localhost:8081/api/v1/volunteers/claim \
	-H 'content-type: application/json' \
	-d '{"volunteer_id":"volunteer-aisha","session_id":"SIM-demo-001"}'
```

The Redis sorted set prioritizes higher risk first and earlier enqueue times first within the same risk level. Claims use atomic Redis operations to remove and record an item. Durable PostgreSQL volunteer bindings arbitrate claim, transfer, and resolve operations across API replicas; migrations `0005` and `0006` retain assignment history while enforcing one active owner per session. Simulated sessions are assigned without calling Twilio. For a live session, the current telephony adapter attempts to redirect its existing Twilio call to the volunteer; that requires an active Twilio Call SID and a configured real recipient number.

## Raft Volunteer Queue

Optional OpenRaft-replicated queue (`SAYANGCARE__QUEUE__BACKEND=raft`); Redis is the default. Step-by-step run commands are in [PHASE_2.md](docs/PHASE_2.md).

## Twilio Webhook Flow

Twilio routes use `application/x-www-form-urlencoded` input and return TwiML XML rather than JSON:

```text
POST /twilio/voice
POST /twilio/gather
POST /twilio/status
POST /twilio/outbound-connect?session_id=<call-sid>
```

Configure the Twilio voice webhook to point to:

```text
https://<public-host>/twilio/voice
```

The intended turn-based voice flow is:

1. `/twilio/voice` creates a session using `CallSid` and returns a `<Gather>` prompt.
2. Twilio listens with `<Gather input="speech">`, transcribes the utterance, then posts `CallSid` and `SpeechResult` to `/twilio/gather`.
3. The handler runs local heuristic risk triage before any model request. High-risk turns bypass Groq, receive a fixed safety response, and are enqueued for volunteer review. Non-crisis turns use Groq, or a deterministic fallback, then return the reply in `<Say>` followed by another `<Gather>`.
4. Groq calls pass through the circuit breaker. If inference is unavailable, SayangCare falls back to a brief deterministic reply; high-risk escalation does not depend on the LLM.
5. `/twilio/status` archives completed or failed calls. The inbound handler registers it on the active call using Twilio's Calls REST API.
6. When a volunteer claims a live escalation, the API creates an outbound Twilio call to that volunteer with `POST /Accounts/{AccountSid}/Calls.json`. The caller leg and volunteer leg join the same conference. `/twilio/outbound-connect` validates the Twilio signature and returns the volunteer leg's conference TwiML.

This is turn-based speech recognition and text-to-speech, not a continuous audio stream. Twilio handles the phone audio and transcription; SayangCare receives form-encoded text and returns TwiML instructions.

For local webhook testing with Cargo, expose the API's actual listening port through a tunnel such as ngrok. Set `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` to the exact public HTTPS origin (without a trailing slash), set real Account SID, Auth Token, and Twilio caller ID values, restart the API, and configure the Twilio number to POST to `https://<public-host>/twilio/voice`. The API validates `X-Twilio-Signature` on all Twilio routes, including the outbound-connect route. A URL or token mismatch returns HTTP 403. Never commit Twilio credentials.

## Storage and Reliability Design

### Hot session store

`RedisSessionStore` stores serialized session JSON under keys like:

```text
session:<session-id>
```

Entries have a one-hour TTL. Updates use a Lua compare-and-swap operation based on the session version, preventing stale concurrent writes from silently overwriting newer state.

### Cold archive

`PostgresArchiveStore` upserts terminal sessions (`completed`, `escalated`, or `failed`) into `sessions`. It stores the transcript and risk assessment as JSONB, preserves timestamps, the monotonic version, handoff and volunteer assignment fields, and the simulated-call flag.

### Tiered storage

`TieredSessionStore` reads Redis first, restores archived sessions from PostgreSQL on a cache miss, and archives terminal sessions before deleting Redis state. Redis CAS and create-if-absent scripts serialize conflicting operations across API pods; process-local locks would not provide that guarantee.

### Circuit breaker

The circuit breaker has three states:

- `Closed`: requests are allowed
- `Open`: requests are rejected and the caller enters degraded handling
- `HalfOpen`: a probe request is allowed after the cooldown

The API stores breaker state and a bounded sample window in Redis, so API replicas share OPEN state, cooldowns, metrics, and a single recovery probe. Three consecutive failures trip immediately; otherwise error-rate and p95 checks use the configured minimum sample count. If Redis cannot coordinate a permit, the API fails closed to inference and uses its fallback path.

## Docker

### Compose services

| Service          |     Host port | Container port | Purpose                                    |
| ---------------- | ------------: | -------------: | ------------------------------------------ |
| `postgres`       |        `5434` |         `5432` | PostgreSQL database                        |
| `redis-master`   |        `6379` |         `6379` | Redis master                               |
| `redis-replica`  | not published |         `6379` | Redis replica                              |
| `redis-sentinel` |       `26379` |        `26379` | Sentinel discovery and failover monitoring |
| `api`            |        `8080` |         `8080` | Containerized SayangCare API               |

Build and start all services:

```bash
docker compose up -d --build
```

The API image uses a multi-stage Rust build. It compiles with Rust `1.88`, copies the release binary and configuration into a Debian runtime image, and exposes port `8080`.

### Docker networking note

The Compose API uses PostgreSQL and Redis Sentinel service hostnames on the Compose network. The session store, Redis queue, and shared circuit-breaker state use the shared Sentinel-aware Redis connection provider.

## Kubernetes

The Kustomize manifests target namespace `sayangcare` and include a three-replica API Deployment/Service, a three-instance CloudNativePG cluster with 5 GiB PVCs and synchronous replication, three Redis nodes, three Sentinel pods, disruption budgets, an HPA, and NGINX Ingress. The local Minikube overlay uses the `sayangcare-api:dev` image; use the production overlay and a published registry image for other clusters. Install the CloudNativePG operator before applying the Kustomization.

### Local Minikube setup (Windows and macOS)

Requirements: Docker Desktop running, `kubectl`, and Minikube. Install Minikube using the package manager for your OS, then start Docker Desktop before creating the cluster.

**macOS (Terminal):**

```bash
brew install minikube
minikube start --driver=docker
kubectl config current-context
kubectl get nodes
```

**Windows (PowerShell):** install Minikube with Chocolatey (`choco install minikube`) or the official Windows installer, then run:

```powershell
minikube start --driver=docker
kubectl config current-context
kubectl get nodes
```

The current local cluster is a single-node Minikube cluster. The setup enabled storage provisioning, Metrics Server, and NGINX Ingress:

```sh
minikube addons enable storage-provisioner
minikube addons enable metrics-server
minikube addons enable ingress
kubectl get storageclass
kubectl top nodes
```

Build and import the API image into Minikube. Re-run these commands after changing API code:

```sh
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev
```

Create a local-only Secret before applying the overlay. Dummy Twilio values are sufficient for dashboard and simulated-call testing; they cannot make live calls. Do not use real values in shell history or commit them:

```sh
kubectl apply -f k8s/base/namespace.yaml
kubectl -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD='local-dev-password' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID='ACtest' --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN='test' --dry-run=client -o yaml | kubectl apply -f -
kubectl -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password='local-dev-password' --dry-run=client -o yaml | kubectl apply -f -
```

Install the CloudNativePG operator and wait for it to be ready:

```sh
kubectl apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl rollout status deployment/cnpg-controller-manager -n cnpg-system
```

Deploy the Minikube stack. The API init container waits for Redis Sentinel and the PostgreSQL primary before starting:

```bash
kubectl apply -k k8s/overlays/minikube
kubectl -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl -n sayangcare rollout status statefulset/redis
kubectl -n sayangcare rollout status statefulset/redis-sentinel

kubectl -n sayangcare rollout status deployment/sayangcare-api
```

Check pods, PVCs, service endpoints, and HPA:

```bash
kubectl -n sayangcare get pods,svc,pvc,hpa
kubectl -n sayangcare get cluster sayangcare-db
kubectl -n sayangcare get endpoints sayangcare-api
kubectl -n sayangcare describe hpa sayangcare-api
kubectl -n sayangcare logs deployment/sayangcare-api
```

For local access, port-forward the Service:

```bash
kubectl -n sayangcare port-forward --address 127.0.0.1 svc/sayangcare-api 18080:80
```

In another terminal, check API health, dependencies, and metrics; open the Call Lab at `http://127.0.0.1:18080/dashboard`:

```bash
curl -i http://127.0.0.1:18080/api/v1/health
curl -i http://127.0.0.1:18080/api/v1/ready
curl http://127.0.0.1:18080/api/v1/metrics
```

In Windows PowerShell, use `curl.exe` rather than the `curl` alias:

```powershell
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/health
curl.exe --noproxy "*" -i http://127.0.0.1:18080/api/v1/ready
curl.exe --noproxy "*" http://127.0.0.1:18080/api/v1/metrics
```

For a pod-recovery smoke test, start a simulated call in the dashboard, note its session ID, delete one API pod, wait for its replacement, and reload that session through the dashboard/API. Redis is shared across the API pods. This checks pod replacement and session retrieval, but does not prove an in-flight Twilio request survives a crash. Minikube has one Kubernetes node, so it cannot validate node-level high availability.

The base HPA uses CPU (65%) and memory (75%). Request-rate autoscaling adds a custom per-pod target (25 requests/second) and requires Prometheus and Prometheus Adapter; install them before applying `kubectl apply -k k8s/overlays/request-rate-hpa`. If the HPA shows `<unknown>` or reports no custom metrics API, request-rate scaling is not active. See [Phase 4 Kubernetes guide](docs/PHASE_4_KUBERNETES.md) for monitoring setup, Ingress/TLS, and external-cluster deployment.

See the [Phase 5 runbook](docs/PHASE_5.md) for starting the local monitoring stack, checking Prometheus/HPA metrics, running a simulated call lifecycle, and tracking the remaining load-test and chaos-engineering work.

The Ingress requires an installed NGINX Ingress Controller, a real hostname, DNS, and a TLS Secret. The checked-in example hostname and certificate name are not ready for public Twilio traffic. CloudNativePG replicates PostgreSQL across three instances, uses one synchronous standby for acknowledged writes, and directs the API to its primary service. Minikube is still a single-node cluster, so all database instances share one node and cannot survive node loss. Configure backups separately; replication is not a backup.

When upgrading an existing populated PostgreSQL deployment, back up and restore its data into the CloudNativePG cluster before switching the API. Kustomize deploys the cluster but does not migrate data; the Phase 4 guide records the tested migration and failover results.

## Observability

Logging uses `tracing` and `tracing-subscriber`. Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=debug,sayangcare=trace cargo run -p sayangcare-api
```

The request-ID middleware adds an `x-request-id` response header and makes the value available to handlers through request extensions.

Current endpoints:

- Health: `/api/v1/health`
- Readiness: `/api/v1/ready`
- Prometheus request counter: `/api/v1/metrics` (`sayangcare_http_requests_total`)

The endpoint exports a process-local cumulative request counter. Prometheus scraping, dashboarding, and HPA custom-metric adapter installation must be configured in the cluster.

## Troubleshooting

### `role "sayangcare" does not exist`

Your command may be connecting to host PostgreSQL on port `5432` or `5433` instead of the Docker database on `5434`.

```bash
unset DATABASE_URL SQLX_DATABASE_URL
docker compose up -d --wait postgres
sqlx migrate run --source migrations
```

Check the endpoint directly:

```bash
set -a
. ./.env
set +a
psql "$DATABASE_URL" \
	-c 'select current_user, current_database();'
```

### Docker Compose warns that `version` is obsolete

Recent Docker Compose versions ignore the top-level `version: "3.9"` field. It is a warning, not a runtime failure. The Compose file can be modernized by removing that field if desired.

### API cannot connect to Redis Sentinel

Check the services and Sentinel logs:

```bash
docker compose ps
docker compose logs redis-master redis-replica redis-sentinel
redis-cli -p 26379 SENTINEL get-master-addr-by-name sayangcare-master
```

For native local Cargo execution, set `SAYANGCARE__REDIS__MASTER_URL` to the host-published Redis URL; use Sentinel discovery for Docker and Kubernetes deployments.

### SQLx compile-time query errors

The repository uses committed offline query metadata. Run:

```bash
cargo check --workspace --all-targets
```

If metadata must be regenerated after changing SQL queries, start PostgreSQL, set `DATABASE_URL` to the Docker database on port `5434`, and run:

```bash
set -a
. ./.env
set +a
cargo sqlx prepare --workspace
```

Review the generated `.sqlx/` files before committing them.

### Port already in use

Inspect listeners:

```bash
lsof -nP -iTCP:5432 -sTCP:LISTEN
lsof -nP -iTCP:5433 -sTCP:LISTEN
lsof -nP -iTCP:5434 -sTCP:LISTEN
lsof -nP -iTCP:8080 -sTCP:LISTEN
```

The intended local arrangement keeps host PostgreSQL on `5432` or `5433` and publishes Docker PostgreSQL on `5434`.

## CB-03 Cached Audio Fallback

Twilio remains the phone gateway. When the `llm-reply` breaker rejects a
request (Open, or a Half-Open probe is already running), `/twilio/gather` returns
a fixed spoken explanation followed by `<Play>` for a bundled six-second chime.
It then gathers speech again, so the next turn can reassess risk and attempt
recovery. The session and transcript are persisted as usual, with state
`degraded`. A successful permitted reply returns the session to `active`.
New calls to `/twilio/voice` also receive this fallback immediately when the
breaker is Open and inference is configured. The greeting checks state without
reserving a Half-Open recovery probe. Retried voice webhooks preserve the stored
session and do not repeat the degraded-state transcript mutation.
High-risk and already escalated calls retain their safety response and volunteer
queue workflow rather than entering audio fallback. Missing AI credentials and
individual failed inference requests retain the existing text fallback; after a
failure opens the breaker, subsequent rejected turns use cached audio.

The Call Lab turn endpoint adds `fallback_audio_url` (a URL during audio fallback,
otherwise `null`) and reports `state: "degraded"`. The dashboard does not
automatically play the audio. The embedded file is available at
`GET /audio/holding-v1.wav`, with `audio/wav` and immutable cache headers; see
[the audio asset notes](crates/api/assets/README.md).

For live calls, `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` must be your public HTTPS
origin and both `/twilio/*` and `/audio/*` must be reachable by Twilio. No new
credentials, audio service, database migration, or runtime asset copy is needed.
Listen locally at `http://localhost:8081/audio/holding-v1.wav` after starting the
API. Automated checks: `cargo test --workspace` and
`cargo check --workspace --all-targets`. Live phone playback still needs Twilio
validation. CB-04 also evaluates sentiment-adjusted thresholds after successful
requests, so slow replies can now open the breaker.

## CB-04 Sentiment-Aware Thresholds

Both the Call Lab and live Twilio turns use the same sentiment workflow. The
API uses separate shared breakers for sentiment scoring and reply generation.
It requests a sentiment signal when that breaker allows it, saves valid output
in `latest_sentiment`, and then generates the supportive reply. High-risk and
already escalated calls continue to bypass inference entirely. A rejected reply
permit invokes CB-03 without making a reply request. If only sentiment scoring
is unavailable, the API clears the old signal, uses deterministic risk severity
for adaptation, and still attempts reply generation.

The current local risk score (1..5) maps to severity using `(score - 1) / 4`:
1 maps to 0, 2 to 0.25, 3 to 0.5, 4 to 0.75, and 5 to 1. The breaker uses
the higher of that severity and the model signal's `distress_severity()`.
LLM sentiment affects degradation thresholds, not the deterministic safety
classifier or volunteer queue priority.

With the existing policy sensitivity of 0.4, each base threshold is multiplied
by `1 - 0.4 * severity`. For example, severity 0.5 changes a 25% error threshold
to 20% and a 2000 ms latency threshold to 1600 ms. Existing policy floors remain
1% for error rate and 50 ms for latency. Each operation evaluates its own shared
Redis sample window. Three consecutive failures open that operation's breaker
without waiting for `MIN_REQUESTS`; mixed outcomes use the configured minimum
sample count and bounded window. Redis atomically reserves one recovery probe
across API replicas. A slow probe reopens its breaker; a healthy probe closes it.

If scoring fails or produces non-finite values, the sentiment breaker records
that operation as failed, clears the stale signal, and uses deterministic risk
severity. A successful reply remains a reply success. Reply failures retain the
deterministic fallback. Without an AI key, local replies continue unchanged. No
database migration or new credentials are required.

This adds one model request per permitted non-crisis turn for sentiment scoring,
in addition to reply generation. Each operation has its own configured AI
timeout, so combined model wait can approach twice that timeout. The shared
breaker adds Redis coordination; if coordination is unavailable, inference is
rejected and the existing fallback is used.

Sentiment is an operational heuristic used only to adjust degradation thresholds.
It is not a clinical assessment and does not decide crisis escalation, which
continues to use deterministic local triage. Neither heuristic is validated for
real crisis care; production use requires independent clinical and safety review.

Verify with `cargo test --workspace` and `cargo check --workspace --all-targets`.
Focused regression checks are `cargo test -p sayangcare-api sentiment::tests`
and `cargo test -p sayangcare-circuit-breaker`. With the API and Groq configured,
send a non-crisis Call Lab turn, then fetch `/api/v1/calls/{sid}` to inspect
`latest_sentiment`. Live Groq scoring and Twilio behavior still need integration
validation.

## Known Limitations

- The Raft queue backend is opt-in (`SAYANGCARE__QUEUE__BACKEND=raft`); Redis remains the default. Raft cluster membership is static, there is no Kubernetes StatefulSet/headless-service manifest for it yet (Phase 4), and Raft state is only persisted when `RAFT_DATA_DIR` is set.
- CB-01 sliding-window metrics and CB-02 recovery transitions/request permits are integrated. CB-03 audio fallback and CB-04 sentiment-adjusted thresholds have local regression coverage; live provider validation remains necessary.
- Groq Chat Completions is the configured inference provider for supportive non-crisis replies. The Groq API key must be configured outside source control. Risk classification remains heuristic and local; high-risk turns bypass the model.
- Twilio request-signature verification, outbound REST dispatch, and the outbound-connect TwiML route are implemented, but still require real Twilio credentials, a public HTTPS URL, a configured caller ID, and live end-to-end validation.
- Volunteer roster, assigned-case mapping, and operator alerts are currently process-local application state, not durable relational bindings shared across API replicas.
- The dashboard simulator tests transcript-based application behavior only; it does not test live microphone audio, Twilio speech recognition, or real phone handoff.
- `/api/v1/ready` verifies Redis and PostgreSQL connectivity, but does not check the configured queue backend or external inference/telephony providers.
- `/api/v1/metrics` exports only a process-local HTTP request counter; it does not expose latency/error histograms or active call counts.
- The Compose Redis setup is a small local demonstration, not a hardened production Redis Sentinel deployment.
- Kubernetes manifests are configured for the local `sayangcare-api:dev` image and still need a registry image, secure secrets, real hostname, DNS, and TLS for another cluster or live Twilio traffic.
- The HPA's CPU/memory metrics require Metrics Server; its request-rate metric requires Prometheus scraping and Prometheus Adapter. HPA operation must be verified in the target cluster.
- The migration is embedded in the API binary and also available through the SQLx CLI; use one controlled migration process in a deployment pipeline to avoid races.

## Development Guidelines

1. Keep domain logic and interfaces in `crates/core`.
2. Implement infrastructure behind the ports in the relevant sibling crate.
3. Keep HTTP parsing and response formatting in `crates/api` handlers.
4. Add or update migrations for schema changes; do not edit an applied migration in a shared database.
5. Regenerate `.sqlx/` metadata when changing `sqlx::query!` statements.
6. Run formatting, workspace checks, and focused tests before opening a pull request:

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace
```

## License

This project is marked as MIT in the workspace Cargo manifest. Confirm the course or team distribution requirements before publishing or redistributing it.
