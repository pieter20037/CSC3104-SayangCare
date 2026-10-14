# Phase 1: Session Lifecycle and Recovery

This guide describes the implemented local session, risk, escalation, and volunteer workflow and how to verify it locally on Windows with PowerShell.

## What Changed

- A call's Twilio `CallSid` is used as the stable session ID instead of generating a different ID for each webhook attempt.
- Session creation is atomic in Redis. Repeating the same CallSid with the same caller returns the existing session; reusing that CallSid for another caller returns HTTP `409 Conflict`.
- Recording caller/assistant turns is a versioned state-machine mutation. Redis compare-and-swap (CAS) rejects stale concurrent writes instead of silently overwriting the latest session state.
- Hangup/status completion commits the terminal state with CAS, archives the full session to PostgreSQL, and deletes the Redis entry only after the archive succeeds.
- A Redis cache miss can restore a nonterminal session, including caller details, transcript, risk, sentiment, state, and version, from PostgreSQL without overwriting a newer Redis entry.
- Migration `0002_add_session_recovery_fields.sql` adds caller display name and latest sentiment without editing the already-applied `0001` migration.
- The JSON Call Lab dashboard creates deterministic simulated sessions, applies local risk triage, and exercises volunteer claiming, transfer, resolve, and acknowledgement without using a real phone call.
- A configured Groq provider can generate supportive non-crisis replies; high-risk turns use the deterministic safety response and are escalated without depending on the LLM.
- CB-01 sliding-window latency/error metrics are merged on `main`; CB-02 recovery transitions and request permits are currently on the remote `phase3-circuitbreaker` branch.
- An orphaned-session sweeper is implemented for inactive Redis sessions; transactional flush retry work remains separate from the live Twilio integration milestone.
- Volunteer claims, transfers, and resolves are stored as durable PostgreSQL assignment records through migrations `0005`, `0006`, and `0007`. A partial unique index permits one active owner per session while retaining resolved assignment history (RAFT-03).
- The volunteer priority queue has two interchangeable backends behind the `PriorityQueue` trait: Redis (default) and an OpenRaft-replicated cluster (RAFT-01) selected with `SAYANGCARE__QUEUE__BACKEND=raft`. Risk 4–5 calls are enqueued on whichever backend is active (RAFT-02).
- The Raft backend has HTTP peer transport (`/internal/raft/*`, shared-secret protected), snapshots, optional on-disk persistence, automatic bootstrap, follower-to-leader write forwarding, and idempotent request IDs.

Redis Lua operations provide cross-process atomicity across API replicas. A process-local `Mutex` or `RwLock` would only coordinate requests within one process and would not protect the same session across Kubernetes pods.

## Requirements

- Docker Desktop is running with the Linux container engine.
- Rust and Cargo are installed.
- The ignored local `.env` exists and has valid PostgreSQL and Redis URLs/credentials. Do not commit `.env`.
- Ports in `.env` are available; this setup uses API `8081`, PostgreSQL `5434`, Redis `6379`, and Sentinel `26379`.

## Start Dependencies

From the repository root, start the local data services:

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

Confirm Sentinel knows the configured master:

```powershell
docker compose exec -T redis-sentinel redis-cli -p 26379 SENTINEL get-master-addr-by-name sayangcare-master
```

The API runs embedded SQLx migrations during startup. Alternatively, apply them with SQLx CLI using the URL from `.env`:

```powershell
$envLine = Get-Content .env | Where-Object { $_ -like 'DATABASE_URL=*' }
$env:DATABASE_URL = $envLine.Substring($envLine.IndexOf('=') + 1)
sqlx migrate run --source migrations
Remove-Item Env:DATABASE_URL
```

After the API starts, verify both migrations:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select version, description, success from _sqlx_migrations order by version;"
```

Expected versions include `1 init`, `2 add session recovery fields`, `3 add volunteer handoff fields`, `4 add simulated session flag`, `5 volunteer assignments`, `6 assignment history and reclaims`, and `7 allow hot session assignments`, all successful.

## Run the API

In terminal 1:

```powershell
cargo run -p sayangcare-api
```

Wait for `HTTP server bound ...8081`. Keep that terminal open. The process stays in the foreground while serving requests. Do not start another `cargo run` on port 8081; stop the running instance with `Ctrl+C` before restarting it.

In terminal 2, check health and readiness:

```powershell
curl.exe -i http://localhost:8081/api/v1/health
curl.exe -i http://localhost:8081/api/v1/ready
```

Both should return HTTP `200`. These are HTTP endpoints, not HTTPS endpoints.

## Exercise the Call Lab

Open `http://localhost:8081/dashboard` in a browser. The dashboard is a local testing console, not a production call-centre UI.

1. Use **Start call** to create a simulated `SIM-...` session.
2. Use **Record turn** with a normal message and confirm the transcript and assistant reply update.
3. Record a high-risk message such as `I want to end my life and need immediate help.` Confirm the deterministic safety reply, elevated risk, escalated state, and pending queue row.
4. Select an on-shift volunteer, such as Aisha or Noor, and use **Claim** or **Claim next**.
5. Open the assigned case, then test **Acknowledge**, **Transfer**, and **Resolve**. Confirm the assignment and alert panels update.
6. Use **End call**, then inspect PostgreSQL as shown below. A high-risk simulation should remain `escalated`, not be forced to `completed`.

The Call Lab uses transcript requests only. It does not use a microphone, place a real phone call, invoke Twilio, or perform a real volunteer handoff.

After claiming a case, verify its durable binding:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select session_id, volunteer_id, risk_level, status from volunteer_assignments order by assigned_at desc limit 5;"
```

Inspect current and historical assignments:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select assignment_id, session_id, volunteer_id, risk_level, status, assigned_at, resolved_at from volunteer_assignments order by assignment_id desc limit 10;"
```

The active owner should have `assigned` status and no `resolved_at`. A transfer closes the prior row and creates a new active row for the next volunteer. **Resolve** closes the active row. A resolved session can later be claimed again, creating a new history row.

## Exercise the Session Lifecycle

Create a call session. Use a new CallSid for each fresh test:

```powershell
$base = 'http://localhost:8081/api/v1'
$callSid = 'CA-phase1-manual-test-001'
$body = @{ call_sid = $callSid; from = '+6591110001'; to = '+6560000000' } | ConvertTo-Json
$first = Invoke-RestMethod -Method Post -Uri "$base/calls/incoming" -ContentType 'application/json' -Body $body
$first
```

Expected: HTTP `200`, with `session_id` equal to the supplied CallSid and initial state `initiated`.

Repeat the exact request to simulate a retried webhook:

```powershell
$retry = Invoke-RestMethod -Method Post -Uri "$base/calls/incoming" -ContentType 'application/json' -Body $body
[pscustomobject]@{ SameSession = ($first.session_id -eq $retry.session_id); State = $retry.state }
```

Expected: `SameSession` is `True`; the original session is not reset.

Try the same CallSid with a different caller:

```powershell
$conflictBody = @{ call_sid = $callSid; from = '+6591999999'; to = '+6560000000' } | ConvertTo-Json
try {
    Invoke-RestMethod -Method Post -Uri "$base/calls/incoming" -ContentType 'application/json' -Body $conflictBody
} catch {
    [int]$_.Exception.Response.StatusCode
}
```

Expected: `409`.

Record a caller turn:

```powershell
$turnBody = @{ transcript = 'I have been feeling isolated.' } | ConvertTo-Json
$turn = Invoke-RestMethod -Method Post -Uri "$base/calls/$callSid/turn" -ContentType 'application/json' -Body $turnBody
$turn
```

Expected: HTTP `200` and a session version greater than the initial version. A stale concurrent write returns `409 Conflict` instead of overwriting a newer version.

Complete and archive the session:

```powershell
Invoke-WebRequest -UseBasicParsing -Method Post -Uri "$base/calls/$callSid/hangup"
```

Expected: HTTP `200`. Verify the transcript was archived and the hot Redis key was removed:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select id, state, version, jsonb_array_length(transcript->'turns') as turns from sessions where id = '$callSid';"
docker compose exec -T redis-master redis-cli EXISTS "session:$callSid"
```

Expected: PostgreSQL shows the session still active in a high-risk escalated state (for the verified sample, `state = escalated`, `version = 3`, `turns = 2`), and Redis returns `0` for the deleted session key. Lower-risk samples may end as `completed`; high-risk sessions intentionally retain `escalated` state for volunteer follow-up.

## Run Automated Checks

```powershell
cargo test --workspace
cargo check --workspace --all-targets
```

This is the order used in the verified smoke pass. The phase-one tests cover stable session IDs, legal state transitions, version increments, full cold-store rehydration, archive-before-delete ordering, and preservation of active state when terminal CAS loses a race. The queue tests cover deterministic risk/FIFO ordering and idempotent command application.

The Raft queue tests run without Docker:

```powershell
cargo test -p sayangcare-priority-queue raft -- --nocapture
```

They cover: the state machine (risk/FIFO ordering, idempotent retries, claim/move/release ownership), a three-node cluster that replicates and survives leader loss, the full `PriorityQueue` contract on a single Raft node, and restart recovery from on-disk state.

### Run a live three-node Raft cluster (RAFT-01/02)

With Docker dependencies running, start three API processes (use three terminals or `Start-Process`); each needs its own port and node ID, the same peer list and token:

```powershell
$env:SAYANGCARE__QUEUE__BACKEND = "raft"
$env:SAYANGCARE__QUEUE__RAFT_PEERS = "1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"
$env:SAYANGCARE__QUEUE__RAFT_TOKEN = "demo-secret"
$env:SAYANGCARE__QUEUE__RAFT_NODE_ID = "1"            # 2 and 3 in the other terminals
$env:SAYANGCARE__SERVER__PORT = "18081"                # 18082 and 18083
$env:SAYANGCARE__QUEUE__RAFT_DATA_DIR = "$env:TEMP\raft\n1"   # optional persistence; n2, n3
cargo run -p sayangcare-api
```

Check roles, then escalate a call through a follower and confirm every node sees it:

```powershell
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/queue/raft; "" }
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/incoming -H "content-type: application/json" -d '{"call_sid":"RAFT-DEMO-1","from":"+6590000000","to":"+6560000000"}'
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/RAFT-DEMO-1/turn -H "content-type: application/json" -d '{"transcript":"I want to kill myself tonight"}'
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/queue; "" }
```

Stop the leader process (`Stop-Process -Id <pid>`), wait about six seconds, and claim through a surviving node; the queue and the volunteer's cases stay consistent on all live nodes, and a restarted node catches up:

```powershell
curl.exe -s -X POST http://127.0.0.1:18083/api/v1/volunteers/claim -H "content-type: application/json" -d '{"volunteer_id":"volunteer-aisha"}'
```

This flow was verified end-to-end: cluster formation, enqueue via a follower, replication to all nodes, leader kill and re-election, claim after failover, and rejoin of the restarted node.

Raft state is stored in memory unless `RAFT_DATA_DIR` is set. Reads are served from the local node's applied state (a write waits for local application, so a node always sees its own writes). Membership is fixed at startup from `RAFT_PEERS`.

The Redis queue integration test is intentionally ignored by default because it requires a live Redis instance. With Docker Desktop running, execute it explicitly:

```powershell
docker compose up -d --wait redis-master
cargo test -p sayangcare-priority-queue --lib -- --include-ignored
```

The workspace currently reports the circuit-breaker, core, priority-queue, session-store, and telephony tests separately. Warnings such as the Redis `get_async_connection` deprecation do not fail the build, but should be cleaned up before final submission.

## Twilio Status

The Twilio webhook routes are implemented, but no Twilio account, phone number, credentials, or webhook URL has been configured. The local API listening on port `8081` is only the application endpoint; it does not by itself connect Twilio. Real webhook testing requires valid Twilio credentials in local `.env`, a publicly reachable HTTPS URL (for example, a configured tunnel), and that URL registered as the Twilio Voice webhook. Do not place credentials in this guide or source control.

## Stop Services

Stop the API with `Ctrl+C` in terminal 1. Stop infrastructure while retaining database data with:

```powershell
docker compose down
```

Do not use `docker compose down -v` unless you intend to delete the local database volume.

