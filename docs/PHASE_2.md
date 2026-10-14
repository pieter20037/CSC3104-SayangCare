# Phase 2: Run Order (Raft Volunteer Queue)

The numbered walkthrough uses PowerShell on Windows. macOS users can use the [Terminal alternative](#macos-terminal-alternative). Both workflows require Docker Desktop running and commands started from the repository root.

## macOS Terminal alternative

Start the shared data services and run the tests in Terminal:

```sh
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
docker compose ps
cargo test --workspace
```

Open three more Terminal windows. Run one node command in each; these environment variables apply only to that terminal.

**Node 1, Terminal 1:**

```sh
export SAYANGCARE__QUEUE__BACKEND=raft
export SAYANGCARE__QUEUE__RAFT_PEERS='1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083'
export SAYANGCARE__QUEUE__RAFT_TOKEN=demo-secret
export SAYANGCARE__QUEUE__RAFT_NODE_ID=1
export SAYANGCARE__SERVER__PORT=18081
export SAYANGCARE__QUEUE__RAFT_DATA_DIR="$TMPDIR/raft/n1"
cargo run -p sayangcare-api
```

**Node 2, Terminal 2:**

```sh
export SAYANGCARE__QUEUE__BACKEND=raft
export SAYANGCARE__QUEUE__RAFT_PEERS='1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083'
export SAYANGCARE__QUEUE__RAFT_TOKEN=demo-secret
export SAYANGCARE__QUEUE__RAFT_NODE_ID=2
export SAYANGCARE__SERVER__PORT=18082
export SAYANGCARE__QUEUE__RAFT_DATA_DIR="$TMPDIR/raft/n2"
cargo run -p sayangcare-api
```

**Node 3, Terminal 3:**

```sh
export SAYANGCARE__QUEUE__BACKEND=raft
export SAYANGCARE__QUEUE__RAFT_PEERS='1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083'
export SAYANGCARE__QUEUE__RAFT_TOKEN=demo-secret
export SAYANGCARE__QUEUE__RAFT_NODE_ID=3
export SAYANGCARE__SERVER__PORT=18083
export SAYANGCARE__QUEUE__RAFT_DATA_DIR="$TMPDIR/raft/n3"
cargo run -p sayangcare-api
```

Leave Terminal 4 for checks. Verify that the nodes agree on a leader and enqueue a test call through a follower:

```sh
for port in 18081 18082 18083; do curl -sS "http://127.0.0.1:$port/api/v1/queue/raft"; done
curl -sS -X POST http://127.0.0.1:18082/api/v1/calls/incoming -H 'content-type: application/json' -d '{"call_sid":"RAFT-DEMO-1","from":"+6590000000","to":"+6560000000"}'
curl -sS -X POST http://127.0.0.1:18082/api/v1/calls/RAFT-DEMO-1/turn -H 'content-type: application/json' -d '{"transcript":"I want to kill myself tonight"}'
for port in 18081 18082 18083; do curl -sS "http://127.0.0.1:$port/api/v1/volunteers/queue"; done
```

Find the leader from the first responses, then press `Ctrl+C` in that node's terminal to stop it. After about six seconds, query the two surviving nodes; they should report the same newly elected leader. Claim the call through a survivor:

```sh
curl -sS http://127.0.0.1:18082/api/v1/queue/raft
curl -sS http://127.0.0.1:18083/api/v1/queue/raft
curl -sS -X POST http://127.0.0.1:18083/api/v1/volunteers/claim -H 'content-type: application/json' -d '{"volunteer_id":"volunteer-aisha"}'
```

Restart the stopped node by rerunning its node command with the same data directory. Confirm it rejoins as a follower and catches up. Stop all three API processes with `Ctrl+C`, then stop the shared services with `docker compose down`.

The checks and demo expectations in the Windows walkthrough apply to macOS too. Use `Ctrl+C` in the leader terminal instead of `Stop-Process`, and `$TMPDIR/raft` instead of `$env:TEMP\raft` for persisted node data.

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

Look out for: every line says `test result: ok`, and no `FAILED`. Two Redis integration tests may show `ignored` when Redis is unavailable; this is expected.

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

Look out for: `HTTP server bound`, `Dashboard: http://localhost:<port>/dashboard` and `Raft priority queue enabled` in each terminal. The first start applies migrations 1–7 automatically.

**Web dashboard.** Each node serves the dashboard at its own link (shown in its startup log):

- Node 1: http://localhost:18081/dashboard
- Node 2: http://localhost:18082/dashboard
- Node 3: http://localhost:18083/dashboard

The **Raft cluster** panel at the top shows every node's state (Leader / Follower / DOWN), term, current leader, log index and applied index, plus whether the cluster still has quorum. It refreshes every 3 seconds, so you can watch the steps below happen in the browser. The same data is available as JSON at `/api/v1/queue/raft/cluster`. If you kill the node you are viewing, the page shows "unreachable"; open another node's link. Restart the API (Ctrl+C, then re-run step 3) after pulling these changes so the new page is served.

## 4. Check the cluster (4th terminal)

```powershell
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/queue/raft; "" }
```

Look out for: all three report the same `current_leader`; exactly one has `"state":"Leader"`, the others `Follower`. Wait ~10 seconds if `current_leader` is `null`.

## 5. Escalate a high-risk call through a follower

```powershell
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/incoming -H "content-type: application/json" -d '{\"call_sid\":\"RAFT-DEMO-1\",\"from\":\"+6590000000\",\"to\":\"+6560000000\"}'
Set-Content -Path $env:TEMP\turn.json -Value '{"transcript":"I want to kill myself tonight"}'
curl.exe -s -X POST http://127.0.0.1:18082/api/v1/calls/RAFT-DEMO-1/turn -H "content-type: application/json" --data-binary "@$env:TEMP\turn.json"
foreach ($p in 18081,18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/queue; "" }
```

Look out for: the turn response has `"risk_level":5`; all three nodes list `RAFT-DEMO-1` in `pending`.

## 6. Kill the leader

Find the leader's process ID (replace `18081` with the leader's port from step 4), then stop it. This simulates a crash.

**Expected:** the leader's terminal (Terminal 1 if node 1 was the leader) will stop and return to a prompt. This is not a bug. `Stop-Process` kills the `cargo run` server running in that terminal, exactly like a real crash. That node stays down until you restart it in step 8.

Run this in Terminal 4, and do not type `<PID>` literally. PowerShell treats `<` as a redirection operator, so use the `$leaderPid` variable instead:

```powershell
$leaderPid = (Get-NetTCPConnection -LocalPort 18081 -State Listen).OwningProcess
Stop-Process -Id $leaderPid
Start-Sleep 6
curl.exe -s http://127.0.0.1:18082/api/v1/queue/raft; ""
curl.exe -s http://127.0.0.1:18083/api/v1/queue/raft; ""
```

Look out for: the two surviving nodes agree on a **new** leader and a higher `current_term`.

**Which node did I kill?** The one that was `"state":"Leader"` in step 4. Its `node_id` tells you the terminal and port: node 1 is Terminal 1 / 18081, node 2 is Terminal 2 / 18082, node 3 is Terminal 3 / 18083. The leader is not always node 1, so use the port from your own step 4 output. You can confirm afterwards:

- Its terminal stops logging and returns to a `PS C:\...>` prompt, while the other two keep running.
- `curl.exe -s http://127.0.0.1:18081/api/v1/queue/raft` (using its port) returns nothing.

Note which terminal it was, because you restart that one in step 8.

## 7. Claim after failover

```powershell
curl.exe -s -X POST http://127.0.0.1:18083/api/v1/volunteers/claim -H "content-type: application/json" -d '{\"volunteer_id\":\"volunteer-aisha\"}'
foreach ($p in 18082,18083) { curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/queue; curl.exe -s http://127.0.0.1:$p/api/v1/volunteers/volunteer-aisha/cases; "" }
```

Look out for: claim returns `"handoff_status":"assigned"`; `pending` is now empty and Aisha's `cases` contains `RAFT-DEMO-1` on both nodes.

## 8. Restart the dead node

The node you killed in step 6 does not restart by itself, so queries to its port return nothing (empty output from `curl -s`) until you bring it back. Re-run the step 3 command for the node you killed, **in that node's own terminal**. Keep the same `RAFT_DATA_DIR`, so the node reloads its saved log and catches up from the current leader.

| Killed node (port) | Terminal to use | Command to re-run                             |
| ------------------ | --------------- | --------------------------------------------- |
| Node 1 (18081)     | Terminal 1      | the **Terminal 1 — Node 1** block from step 3 |
| Node 2 (18082)     | Terminal 2      | the **Terminal 2 — Node 2** block from step 3 |
| Node 3 (18083)     | Terminal 3      | the **Terminal 3 — Node 3** block from step 3 |

For example, if you killed node 1, paste this into **Terminal 1**:

```powershell
$env:SAYANGCARE__QUEUE__BACKEND="raft"; $env:SAYANGCARE__QUEUE__RAFT_PEERS="1=http://127.0.0.1:18081,2=http://127.0.0.1:18082,3=http://127.0.0.1:18083"; $env:SAYANGCARE__QUEUE__RAFT_TOKEN="demo-secret"; $env:SAYANGCARE__QUEUE__RAFT_NODE_ID="1"; $env:SAYANGCARE__SERVER__PORT="18081"; $env:SAYANGCARE__QUEUE__RAFT_DATA_DIR="$env:TEMP\raft\n1"; cargo run -p sayangcare-api
```

Leave that terminal running. Then run the checks below in **Terminal 4**. If you killed a different node, change the port in the checks to match.

Wait for `HTTP server bound` (it may recompile first), then in Terminal 4:

```powershell
curl.exe -s http://127.0.0.1:18081/api/v1/queue/raft; ""
curl.exe -s http://127.0.0.1:18081/api/v1/volunteers/volunteer-aisha/cases
```

Look out for: it comes back as `Follower` with `current_leader` set to the new leader, and `last_applied` / `last_log_index` match the other nodes, higher than before the kill because of the step 7 claim. Aisha's case is still there. It does not take leadership back, because the cluster already has a leader.

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
- Port already in use: an old API is still running. Find it with `Get-NetTCPConnection -LocalPort 18081 -State Listen` (use the port in question).
- Empty output from `curl.exe -s` on a node's port: that node is not running (for example, you killed it in step 6). Restart it as in step 8.
- `The '<' operator is reserved for future use`: you pasted a `<placeholder>` literally. Use the `$leaderPid` form shown in step 6.
- `current_leader` stays `null`: peers can't reach each other. Check all three nodes use the identical `RAFT_PEERS` and `RAFT_TOKEN`.
- Old state confusing a fresh run: delete `$env:TEMP\raft` before starting the cluster.
- Single-node, no Raft: skip steps 3–8, run `cargo run -p sayangcare-api` with `SAYANGCARE__QUEUE__BACKEND` unset (Redis) or set to `raft`.
