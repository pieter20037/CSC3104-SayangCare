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

The current codebase is a working scaffold. The JSON Call Lab simulator and live Twilio `/gather` path apply deterministic risk triage and can generate supportive replies through Groq's OpenAI-compatible Chat Completions API. High-risk turns bypass the LLM and use a fixed safety response before queue escalation. Without a Groq API key, both paths use deterministic fallback replies. Metrics and readiness are intentionally minimal; see [Known Limitations](#known-limitations).

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
- [Phase 1 Test Guide](PHASE_1_TEST_GUIDE.md)
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
	+--> PostgreSQL --> durable completed sessions and transcripts
	|
	+--> adaptive circuit breaker --> degraded mode / escalation behavior
```

The API is built as a Cargo workspace. The `core` crate owns domain types and ports; infrastructure crates implement those ports; the `api` crate assembles everything and exposes HTTP routes.

## Repository Layout

| Path                     | Responsibility                                                         |
| ------------------------ | ---------------------------------------------------------------------- |
| `Cargo.toml`             | Workspace members, shared dependencies, Rust version, release profile  |
| `crates/core`            | Domain models, configuration, errors, and hexagonal-architecture ports |
| `crates/api`             | Actix Web binary, application bootstrap, routes, middleware, telemetry |
| `crates/session-store`   | Redis hot store, PostgreSQL archive store, and tiered storage facade   |
| `crates/priority-queue`  | Redis sorted-set queue and atomic volunteer claims                     |
| `crates/circuit-breaker` | Circuit states, rolling counters, adaptive thresholds, registry        |
| `crates/telephony`       | Twilio REST API implementation of the telephony port                   |
| `migrations/`            | SQLx PostgreSQL migrations                                             |
| `config/default.toml`    | Default non-secret application configuration                           |
| `docker-compose.yml`     | Local PostgreSQL, Redis, Redis Sentinel, and API services              |
| `Dockerfile`             | Multi-stage release image build                                        |
| `k8s/`                   | Kubernetes namespace, configuration, workloads, probes, and HPA        |
| `.sqlx/`                 | Committed SQLx offline query metadata                                  |

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

`cargo fmt --check` checks formatting, `cargo check --workspace --all-targets` compiles the workspace and test targets, and `cargo test --workspace` runs automated tests. These commands do not prove Docker services or real Twilio connectivity work. For live local service checks, follow Quick Start and use the smoke-test flow in [PHASE_1_TEST_GUIDE.md](PHASE_1_TEST_GUIDE.md). In the verified local smoke test, the same CallSid reused successfully, a conflicting caller returned `409`, a high-risk transcript escalated the session, and the Redis hot key was removed after the archive check.

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

| Setting                                             | Local example                          | Purpose                                                             |
| --------------------------------------------------- | -------------------------------------- | ------------------------------------------------------------------- |
| `SAYANGCARE__SERVER__HOST`                          | `0.0.0.0`                              | HTTP bind address                                                   |
| `SAYANGCARE__SERVER__PORT`                          | `8081`                                 | HTTP port                                                           |
| `SAYANGCARE__SERVER__WORKERS`                       | `4`                                    | Actix worker count                                                  |
| `SAYANGCARE__POSTGRES__URL`                         | `...@127.0.0.1:5434/...`               | PostgreSQL connection URL                                           |
| `SAYANGCARE__POSTGRES__MAX_CONNECTIONS`             | `20`                                   | PostgreSQL pool limit                                               |
| `SAYANGCARE__REDIS__SENTINEL_ENDPOINTS`             | `["127.0.0.1:26379"]`                  | Sentinel endpoint list                                              |
| `SAYANGCARE__REDIS__MASTER_NAME`                    | `sayangcare-master`                    | Sentinel master name                                                |
| `SAYANGCARE__REDIS__MASTER_URL`                     | `redis://127.0.0.1:6379/`              | Optional direct session-store URL; set for native local development |
| `SAYANGCARE__REDIS__QUEUE_URL`                      | `redis://127.0.0.1:6379/`              | Redis endpoint used by the priority queue                           |
| `SAYANGCARE__REDIS__PASSWORD`                       | unset locally                          | Redis password, if enabled                                          |
| `SAYANGCARE__CIRCUIT_BREAKER__BASE_ERROR_RATE`      | `0.25`                                 | Base error threshold                                                |
| `SAYANGCARE__CIRCUIT_BREAKER__BASE_LATENCY_MS`      | `2000`                                 | Base latency threshold                                              |
| `SAYANGCARE__CIRCUIT_BREAKER__MIN_REQUESTS`         | `20`                                   | Minimum samples before evaluation                                   |
| `SAYANGCARE__CIRCUIT_BREAKER__HALF_OPEN_AFTER_SECS` | `10`                                   | Open-to-half-open delay                                             |
| `SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID`         | test value locally                     | Twilio account identifier                                           |
| `SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN`          | test value locally                     | Twilio credential                                                   |
| `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL`            | `http://localhost:8081`                | Public URL used in TwiML actions                                    |
| `SAYANGCARE__AI__API_KEY`                           | unset                                  | Optional Groq API key; keep it in ignored `.env`                    |
| `SAYANGCARE__AI__MODEL`                             | `allam-2-7b`                            | Groq-hosted chat model                                              |
| `SAYANGCARE__AI__BASE_URL`                          | Groq Chat Completions URL              | OpenAI-compatible chat completions endpoint                         |
| `SAYANGCARE__AI__TIMEOUT_SECS`                      | `12`                                   | Inference request timeout                                           |
| `RUST_LOG`                                          | `info,sayangcare=debug,actix_web=info` | Log filter                                                          |

Never commit real Twilio, LLM, database, or Redis credentials. Use Kubernetes Secrets or an external secret manager for deployed environments.

To enable Groq locally, put your Groq key in the ignored `.env` file as `SAYANGCARE__AI__API_KEY=...`, set the model and base URL shown in `.env.example`, then restart the API. Do not paste the key into source files, README, or chat. If the key is absent or a request fails, the API uses a deterministic fallback. The model generates supportive non-crisis replies; local heuristic triage remains responsible for immediate escalation.

## Database and Migrations

The migration in `migrations/0001_init.sql` creates:

- `sessions`: durable session state and transcripts
- `volunteers`: volunteer registry
- `escalations`: auditable session-to-volunteer assignments
- Indexes for caller, creation time, state, and open escalations

The API also runs embedded migrations during startup with `sqlx::migrate!`, so a running API expects the database to be reachable and migration-compatible.

### Important local port distinction

| Context                                     | PostgreSQL address                           |
| ------------------------------------------- | -------------------------------------------- |
| Host shell, `sqlx`, `psql`, local Cargo API | `127.0.0.1:5434`                             |
| API container in Compose                    | `postgres:5432`                              |
| Kubernetes API pod                          | `postgres.sayangcare.svc.cluster.local:5432` |

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

`/health` returns service status and package version. `/ready` currently returns `{ "ready": true }` without checking dependencies. `/metrics` is a placeholder Prometheus-compatible text response.

### Call session routes

```text
POST /api/v1/calls/incoming
GET  /api/v1/calls/{session_id}
POST /api/v1/calls/{session_id}/turn
POST /api/v1/calls/{session_id}/hangup
```

For the browser-based Call Lab, start the API and open `http://localhost:<api-port>/dashboard`. Use the port shown in the API startup log; it depends on the effective configuration (the native-development examples in this README use `8081`, while Compose publishes `8080`).

### Call Lab simulator

The **Simulate crisis call** action creates a `SIM-...` session, records a low-risk opening turn followed by the escalation statement entered in the form, and leaves an escalated case pending in the volunteer queue. With a Groq key configured, non-crisis turns receive a generated supportive reply; high-risk turns use the deterministic safety response and local heuristic classification. Without a key, all turns use deterministic fallback replies. These are test transcripts, not microphone or phone audio.

The queue's row-level **Claim** action claims that specific session for Aisha by default. A roster **Claim as <name>** action claims the highest-priority pending session for that volunteer. The default on-shift roster includes Aisha and Noor; Ibrahim is off shift. A claimed case appears under the volunteer's assigned cases. **Transfer** moves an assigned case to another on-shift volunteer (selected automatically); **Resolve** closes the volunteer handoff; **Acknowledge** records operator review. Simulated calls update SayangCare state only and never redirect a real phone call.

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

Turn batches increment the session version once and persist through Redis Lua compare-and-swap. Concurrent updates with a stale version return `409 Conflict`. Completion also uses CAS before archiving the full session to PostgreSQL; Redis state is deleted only after the durable archive succeeds. On a cache miss, nonterminal archived sessions are restored to Redis only if no concurrent request has already recreated them.

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
	-d '{"volunteer_id":"volunteer-001"}'
```

To claim one particular pending session, include its ID:

```bash
curl -X POST http://localhost:8081/api/v1/volunteers/claim \
	-H 'content-type: application/json' \
	-d '{"volunteer_id":"volunteer-aisha","session_id":"SIM-demo-001"}'
```

The Redis sorted set prioritizes higher risk first and earlier enqueue times first within the same risk level. Claims use atomic Redis operations to remove and record an item. Simulated sessions are assigned without calling Twilio. For a live session, the current telephony adapter attempts to redirect its existing Twilio call to the volunteer; that requires an active Twilio Call SID and a configured real recipient number.

## Twilio Webhook Flow

Twilio routes use `application/x-www-form-urlencoded` input and return TwiML XML rather than JSON:

```text
POST /twilio/voice
POST /twilio/gather
POST /twilio/status
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
5. `/twilio/status` can archive a completed or failed session, but the current inbound TwiML flow does not attach that status callback to the Twilio call.

This is turn-based speech recognition and text-to-speech, not a continuous audio stream. Twilio handles the phone audio and transcription; SayangCare receives form-encoded text and returns TwiML instructions.

For local webhook testing with Cargo, expose the API's actual listening port through a tunnel such as ngrok, then set `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` to the public HTTPS URL and configure the Twilio number to POST to `https://<public-host>/twilio/voice`. The current project does not yet verify `X-Twilio-Signature`; do not treat the webhook as production-secure or expose it for untrusted use. Never commit Twilio credentials.

## Storage and Reliability Design

### Hot session store

`RedisSessionStore` stores serialized session JSON under keys like:

```text
session:<session-id>
```

Entries have a one-hour TTL. Updates use a Lua compare-and-swap operation based on the session version, preventing stale concurrent writes from silently overwriting newer state.

### Cold archive

`PostgresArchiveStore` upserts completed sessions into `sessions`. It stores the transcript and risk assessment as JSONB and preserves timestamps and the monotonic version.

### Tiered storage

`TieredSessionStore` reads Redis first, restores full nonterminal sessions from PostgreSQL on a cache miss, and archives completed sessions before deleting Redis state. Redis CAS and create-if-absent scripts serialize conflicting operations across API pods; process-local locks would not provide that guarantee.

### Circuit breaker

The circuit breaker has three states:

- `Closed`: requests are allowed
- `Open`: requests are rejected and the caller enters degraded handling
- `HalfOpen`: a probe request is allowed after the cooldown

The default background ticker checks breakers every five seconds. The breaker module has configurable states and adaptive thresholds, but API handlers do not yet record real inference success/failure measurements or call a concrete inference service.

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

The Compose API uses PostgreSQL, Sentinel, and Redis service hostnames on the Compose network. Its queue endpoint is configured separately as `redis://redis-master:6379/`; the session store continues to discover the master through Sentinel.

## Kubernetes

The manifests target a namespace called `sayangcare` and include:

- A namespace and shared ConfigMap in `k8s/namespace.yaml`
- A three-replica API Deployment and ClusterIP Service in `k8s/app-deployment.yaml`
- A PostgreSQL StatefulSet with a 5 GiB PVC in `k8s/postgres.yaml`
- Three Redis pods plus three Sentinel pods in `k8s/redis-sentinel.yaml`
- An HPA targeting CPU and `active_voice_sessions` in `k8s/hpa.yaml`

Create the namespace/configuration, then create the Secret from your ignored `.env` file:

```bash
kubectl apply -f k8s/namespace.yaml
kubectl create secret generic sayangcare-secrets -n sayangcare \
	--from-env-file=.env --dry-run=client -o yaml | kubectl apply -f -
```

Before deploying, replace the placeholder image in `k8s/app-deployment.yaml`:

```text
ghcr.io/your-org/sayangcare-api:latest
```

Then deploy:

```bash
kubectl apply -f k8s/postgres.yaml
kubectl apply -f k8s/redis-sentinel.yaml
kubectl apply -f k8s/app-deployment.yaml
kubectl apply -f k8s/hpa.yaml -n sayangcare
```

Inspect rollout and logs:

```bash
kubectl get pods -n sayangcare
kubectl rollout status deployment/sayangcare-api -n sayangcare
kubectl logs -n sayangcare deployment/sayangcare-api -f
kubectl get hpa -n sayangcare
```

The API Service is internal to the cluster. Use port forwarding for local access:

```bash
kubectl port-forward -n sayangcare service/sayangcare-api 8080:80
```

The Kubernetes files are deployment templates, not a complete production installation. Review image names, secret management, storage classes, ingress/TLS, Redis authentication, and migration execution before production use.

## Observability

Logging uses `tracing` and `tracing-subscriber`. Set `RUST_LOG` to control verbosity:

```bash
RUST_LOG=debug,sayangcare=trace cargo run -p sayangcare-api
```

The request-ID middleware adds an `x-request-id` response header and makes the value available to handlers through request extensions.

Current endpoints:

- Health: `/api/v1/health`
- Readiness: `/api/v1/ready`
- Metrics placeholder: `/api/v1/metrics`

The metrics endpoint is not yet exporting real counters. The Kubernetes Prometheus annotations are present for future integration.

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

## Known Limitations

- The Raft priority-queue command model and OpenRaft type configuration are implemented and unit-tested, but `AppState` still uses `RedisPriorityQueue`. Raft peer transport, cluster membership/bootstrap, leader forwarding, and runtime state-machine/log integration are not implemented yet; do not describe the running service as having a Raft-backed HA queue.
- Groq Chat Completions is the configured inference provider for supportive non-crisis replies. The Groq API key must be configured outside source control. Risk classification remains heuristic and local; high-risk turns bypass the model.
- Twilio request-signature verification (`X-Twilio-Signature`) is not implemented.
- The telephony adapter updates an existing call for redirects; outbound volunteer dispatch using `POST /Calls.json` and a dedicated outbound-connect TwiML route are not implemented.
- Volunteer roster, assigned-case mapping, and operator alerts are currently process-local application state, not durable relational bindings shared across API replicas.
- The status callback route exists, but the incoming-call TwiML does not currently configure Twilio to invoke it.
- The dashboard simulator tests transcript-based application behavior only; it does not test live microphone audio, Twilio speech recognition, or real phone handoff.
- `/api/v1/ready` does not verify PostgreSQL or Redis connectivity.
- `/api/v1/metrics` returns placeholder text rather than real Prometheus metrics.
- The Compose Redis setup is a small local demonstration, not a hardened production Redis Sentinel deployment.
- Kubernetes manifests contain placeholder image and secret values and require environment-specific review.
- The HPA references an `active_voice_sessions` custom metric that requires a metrics adapter and an actual exporter.
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
