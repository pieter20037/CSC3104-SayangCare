# CSC3104-SayangCare

Running code #For Darren's Reference
cargo run -p sayangcare-api

Second Terminal Curl to see it's working
curl.exe http://localhost:8081/api/v1/health

SayangCare is a Rust-based, voice-first telehealth service for handling inbound calls, maintaining conversation state, detecting distress, and escalating high-risk sessions to human volunteers.

This repository is the CSC3104 cloud-computing project. It demonstrates a modular backend with:

- Actix Web HTTP and Twilio webhook endpoints
- Redis Sentinel for hot, replicated session state
- PostgreSQL for durable session archives and audit data
- A Redis-backed priority queue for volunteer escalation
- An adaptive circuit breaker for degraded-mode behavior
- Docker Compose for local infrastructure
- Kubernetes manifests for a multi-replica deployment

The current codebase is a working scaffold. The LLM inference port exists in the domain layer, but the HTTP flow currently uses a placeholder assistant response. Metrics and readiness are also intentionally minimal; see [Known Limitations](#known-limitations).

## Contents

- [Architecture](#architecture)
- [Repository Layout](#repository-layout)
- [Requirements](#requirements)
- [Quick Start: Local Development](#quick-start-local-development)
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

Install the following tools:

- Rust `1.88` or newer, including Cargo
- Docker Desktop with Docker Compose
- SQLx CLI, preferably matching the SQLx major version used by the project
- `psql` is optional but useful for database inspection
- `curl` for endpoint checks
- `kubectl` only if deploying to Kubernetes

Install SQLx CLI if necessary:

```bash
cargo install sqlx-cli --no-default-features --features postgres,rustls
```

Check the toolchain:

```bash
rustc --version
cargo --version
docker --version
docker compose version
sqlx --version
```

## Quick Start: Local Development

### 1. Create local configuration

The repository ignores `.env` because it may contain credentials. Create it from the example if it does not exist:

```bash
cp .env.example .env
```

Set your PostgreSQL password and Twilio credentials in `.env` before starting the API or Compose. The sample values are placeholders for local development; do not use them for a deployed service. The API requires the database URL and Twilio credentials from environment variables rather than `config/default.toml`.

The checked-in local defaults use:

- PostgreSQL: `127.0.0.1:5434`
- Redis Sentinel: `127.0.0.1:26379`
- API: `http://localhost:8081`

Port `5434` is intentional. It avoids conflicts with host PostgreSQL services that may already use ports `5432` or `5433`.

### 2. Start local infrastructure

For the recommended local workflow, run the databases and Redis services but run the Rust API from Cargo:

```bash
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
```

Check service status:

```bash
docker compose ps
```

### 3. Apply migrations

Clear shell-level overrides if you previously exported an old `DATABASE_URL`:

```bash
unset DATABASE_URL SQLX_DATABASE_URL
sqlx migrate run --source migrations
```

Confirm the migration state:

```bash
sqlx migrate info --source migrations
```

Expected output includes:

```text
1/installed init
2/installed add session recovery fields
```

### 4. Build and run the API

```bash
cargo run -p sayangcare-api
```

The local `.env` listens on `http://localhost:8081`, overriding the `8080` port in `config/default.toml`. The API loads `.env`, `config/default.toml`, and `SAYANGCARE__...` environment overrides during startup.

For native Windows/macOS/Linux development, `.env.example` configures `SAYANGCARE__REDIS__MASTER_URL` to use the host-published Redis port directly. Docker Sentinel advertises a container-private master address that a process running on the host cannot reach. Leave `MASTER_URL` unset for Compose or Kubernetes so the session store discovers the master through Sentinel.

In a second terminal, check the service:

```bash
curl http://localhost:8081/api/v1/health
curl http://localhost:8081/api/v1/ready
```

Stop local infrastructure when finished:

```bash
docker compose down
```

The PostgreSQL and Redis data volumes are retained by default. To remove them as well, use the destructive command:

```bash
docker compose down -v
```

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
| `RUST_LOG`                                          | `info,sayangcare=debug,actix_web=info` | Log filter                                                          |

Never commit real Twilio, LLM, database, or Redis credentials. Use Kubernetes Secrets or an external secret manager for deployed environments.

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
POST /api/v1/calls/{session_id}/turn
POST /api/v1/calls/{session_id}/hangup
```

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
POST /api/v1/volunteers/claim
```

Claim the highest-priority queued session:

```bash
curl -X POST http://localhost:8081/api/v1/volunteers/claim \
	-H 'content-type: application/json' \
	-d '{"volunteer_id":"volunteer-001"}'
```

The Redis sorted set prioritizes higher risk first and earlier enqueue times first within the same risk level. Claims use a Lua script to remove and record an item atomically.

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

The flow is:

1. `/twilio/voice` creates a session using `CallSid` and returns a `<Gather>` prompt.
2. Twilio posts `CallSid` and `SpeechResult` to `/twilio/gather`.
3. The handler records the turn and returns a placeholder assistant response.
4. If the circuit breaker is open, the handler returns holding audio and can enqueue high-risk sessions.
5. `/twilio/status` marks completed or failed calls as completed and archives them.

For local webhook testing with Cargo, expose port `8081` through a tunnel such as ngrok, then set `SAYANGCARE__TELEPHONY__PUBLIC_BASE_URL` to the public HTTPS URL. Do not expose test credentials in source control.

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

The default background ticker checks breakers every five seconds. Thresholds are adapted using distress severity, although the current HTTP scaffold does not yet connect a real inference service to sentiment and risk scoring.

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
- The LLM inference port is defined, but no concrete inference provider is wired into the API. `/twilio/gather` returns a fixed placeholder reply.
- Sentiment and risk scoring are domain concepts but are not currently populated by a live inference service.
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
