# Phase 2: Run Order (Raft Volunteer Queue)

Run everything from the repo root in PowerShell. Needs Docker Desktop running.

## 1. Start dependencies

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
```

Look out for: `postgres` shows `healthy`; ports `5434` (Postgres), `6379` (Redis), `26379` (Sentinel) are free.

## 2. Build and test

```powershell
cargo test --workspace
```

Look out for: every line says `test result: ok`, and no `FAILED`. One Redis test shows `1 ignored` (normal).

## 3. Start a 3-node Raft cluster

Open **four** PowerShell terminals in the repo root. Copy each command into its own terminal. Start node 1 first.

**Terminal 1 — Node 1**

```powershell
$env:SAYANGCARE__QUEUE__BACKEND="raft"; $env:SAYANGCARE__QUEUE__RAFT_PEERS="1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"; $env:SAYANGCARE__QUEUE__RAFT_TOKEN="demo-secret"; $env:SAYANGCARE__QUEUE__RAFT_NODE_ID="1"; $env:SAYANGCARE__SERVER__PORT="18081"; $env:SAYANGCARE__QUEUE__RAFT_DATA_DIR="$env:TEMP\raft\n1"; cargo run -p sayangcare-api
```

**Terminal 2 — Node 2**

```powershell
$env:SAYANGCARE__QUEUE__BACKEND="raft"; $env:SAYANGCARE__QUEUE__RAFT_PEERS="1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"; $env:SAYANGCARE__QUEUE__RAFT_TOKEN="demo-secret"; $env:SAYANGCARE__QUEUE__RAFT_NODE_ID="2"; $env:SAYANGCARE__SERVER__PORT="18082"; $env:SAYANGCARE__QUEUE__RAFT_DATA_DIR="$env:TEMP\raft\n2"; cargo run -p sayangcare-api
```

**Terminal 3 — Node 3**

```powershell
$env:SAYANGCARE__QUEUE__BACKEND="raft"; $env:SAYANGCARE__QUEUE__RAFT_PEERS="1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"; $env:SAYANGCARE__QUEUE__RAFT_TOKEN="demo-secret"; $env:SAYANGCARE__QUEUE__RAFT_NODE_ID="3"; $env:SAYANGCARE__SERVER__PORT="18083"; $env:SAYANGCARE__QUEUE__RAFT_DATA_DIR="$env:TEMP\raft\n3"; cargo run -p sayangcare-api
```

**Terminal 4 — leave free for the checks in steps 4–9.**

Look out for: `HTTP server bound` and `Raft priority queue enabled` in each terminal. The first start applies migrations 1–7 automatically.

## 4. Check the cluster (4th terminal)

```powershell
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/queue/raft; "" }
```

Look out for: all three report the same `current_leader`; exactly one has `"state":"Leader"`, the others `Follower`. Wait ~10 seconds if `current_leader` is `null`.

## 5. Escalate a high-risk call through a follower

```powershell
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/incoming -H "content-type: application/json" -d '{"call_sid":"RAFT-DEMO-1","from":"+6590000000","to":"+6560000000"}'
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/RAFT-DEMO-1/turn -H "content-type: application/json" -d '{"transcript":"I want to kill myself tonight"}'
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/queue; "" }
```

Look out for: the turn response has `"risk_level":5`; all three nodes list `RAFT-DEMO-1` in `pending`.

## 6. Kill the leader

Find the leader's process ID (replace `18081` with the leader's port from step 4), then stop it:

```powershell
(Get-NetTCPConnection -LocalPort 18081 -State Listen).OwningProcess
Stop-Process -Id <PID>
Start-Sleep 6
curl.exe -s http://127.0.0.1:18082/api/v1/queue/raft; ""
curl.exe -s http://127.0.0.1:18083/api/v1/queue/raft; ""
```

Look out for: the two surviving nodes agree on a **new** leader and a higher `current_term`.

## 7. Claim after failover

```powershell
curl.exe -s -X POST http://127.0.0.1:18083/api/v1/volunteers/claim -H "content-type: application/json" -d '{"volunteer_id":"volunteer-aisha"}'
foreach ($p in 18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/queue; curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/volunteer-aisha/cases; "" }
```

Look out for: claim returns `"handoff_status":"assigned"`; `pending` is now empty and Aisha's `cases` contains `RAFT-DEMO-1` on both nodes.

## 8. Restart the dead node

Re-run the step 3 block in the stopped node's terminal, then:

```powershell
curl.exe -s http://127.0.0.1:18081/api/v1/queue/raft; ""
curl.exe -s http://127.0.0.1:18081/api/v1/volunteers/volunteer-aisha/cases
```

Look out for: it comes back as `Follower` and `last_applied` catches up with the others; Aisha's case is still there.

## 9. Verify the database (RAFT-03)

```powershell
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c "select version, description, success from _sqlx_migrations order by version;"
```

Look out for: versions 1–7 all `success = t`, including `7 allow hot session assignments`.

## 10. Shut down

`Ctrl+C` in each API terminal, then:

```powershell
docker compose down
```

Do not use `-v` unless you want to delete the database.

## Troubleshooting

- `role "sayangcare" does not exist`: a stale `DATABASE_URL` points at another Postgres. Run `Remove-Item Env:DATABASE_URL`.
- Port already in use: an old API is still running. Find it with `Get-NetTCPConnection -LocalPort <port> -State Listen`.
- `current_leader` stays `null`: peers can't reach each other. Check all three nodes use the identical `RAFT_PEERS` and `RAFT_TOKEN`.
- Old state confusing a fresh run: delete `$env:TEMP\raft` before starting the cluster.
- Single-node, no Raft: skip steps 3–8, run `cargo run -p sayangcare-api` with `SAYANGCARE__QUEUE__BACKEND` unset (Redis) or set to `raft`.
