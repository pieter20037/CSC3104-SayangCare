# Phase 1: Session Lifecycle and Recovery

This guide describes the phase-one session lifecycle work and how to verify it locally on Windows with PowerShell.

## What Changed

- A call's Twilio `CallSid` is used as the stable session ID instead of generating a different ID for each webhook attempt.
- Session creation is atomic in Redis. Repeating the same CallSid with the same caller returns the existing session; reusing that CallSid for another caller returns HTTP `409 Conflict`.
- Recording caller/assistant turns is a versioned state-machine mutation. Redis compare-and-swap (CAS) rejects stale concurrent writes instead of silently overwriting the latest session state.
- Hangup/status completion commits the terminal state with CAS, archives the full session to PostgreSQL, and deletes the Redis entry only after the archive succeeds.
- A Redis cache miss can restore a nonterminal session, including caller details, transcript, risk, sentiment, state, and version, from PostgreSQL without overwriting a newer Redis entry.
- Migration `0002_add_session_recovery_fields.sql` adds caller display name and latest sentiment without editing the already-applied `0001` migration.

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

Expected versions include `1 init` and `2 add session recovery fields`, both successful.

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

Expected: HTTP `200`. Verify the full transcript was archived and the hot Redis key was removed:

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select id, state, version, jsonb_array_length(transcript->'turns') as turns from sessions where id = '$callSid';"
docker compose exec -T redis-master redis-cli EXISTS "session:$callSid"
```

Expected: PostgreSQL shows state `completed`, an incremented version, and one caller turn. Redis returns `0` for the deleted session key.

## Run Automated Checks

```powershell
cargo test --workspace
cargo check --workspace --all-targets
```

The phase-one tests cover stable session IDs, legal state transitions, version increments, full cold-store rehydration, archive-before-delete ordering, and preservation of active state when terminal CAS loses a race.

## Twilio Status

The Twilio webhook routes are implemented, but no Twilio account, phone number, credentials, or webhook URL has been configured. The local API listening on port `8081` is only the application endpoint; it does not by itself connect Twilio. Real webhook testing requires valid Twilio credentials in local `.env`, a publicly reachable HTTPS URL (for example, a configured tunnel), and that URL registered as the Twilio Voice webhook. Do not place credentials in this guide or source control.

## Stop Services

Stop the API with `Ctrl+C` in terminal 1. Stop infrastructure while retaining database data with:

```powershell
docker compose down
```

Do not use `docker compose down -v` unless you intend to delete the local database volume.
