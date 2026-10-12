# Raft Leader Election Demo (Dashboard)

Shows the Raft volunteer queue surviving a leader crash. Needs Docker Desktop. Run from the repo root.

## Setup (once)

```powershell
docker compose up -d --wait postgres redis-master redis-replica redis-sentinel
Remove-Item -Recurse -Force $env:TEMP\raft -ErrorAction SilentlyContinue
```

Start three nodes, one per terminal:

```powershell
.\scripts\start-raft-node.ps1 1
.\scripts\start-raft-node.ps1 2
.\scripts\start-raft-node.ps1 3
```

Wait for `Raft priority queue enabled` in each.

## Demo steps

1. Open http://localhost:18082/dashboard. The **Raft cluster** panel (refreshes every 3s) should read `3/3 up`, with exactly one **Leader** and two **Followers**, all on `term 1`.
2. Click **Simulate crisis call**. The Session record shows risk 5/5, ESCALATED. This writes to the Raft log; every node's `log` and `applied` index goes up together.
3. **Kill the leader** (the node labelled Leader on the dashboard). In a 4th terminal, using that node's port (18081, 18082 or 18083):

   ```powershell
   (Get-NetTCPConnection -LocalPort 18081 -State Listen).OwningProcess
   Stop-Process -Id 11860   # paste the PID printed above
   ```

   Do not use the leader's own dashboard tab afterwards, because it will be unreachable. Use a surviving node's link.
4. Watch the panel (about 5-10 seconds): the dead node shows **DOWN**, the summary reads `2/3 up`, a survivor becomes **Leader**, and `term` increases (1 to 2). Quorum stays OK (2 of 3), so writes still work.
5. Click **Simulate crisis call** again to prove the cluster still accepts writes with the new leader.
6. **Restart the dead node** by re-running its start command (for example `.\scripts\start-raft-node.ps1 1`). It returns as a **Follower**, with the same term and log index as the others. It does not take leadership back.

## What to say

- Raft elects one leader per term. Followers that stop hearing the leader's heartbeat start an election, and a candidate needs a majority (2 of 3) of votes.
- Killing the leader raises the term and elects a new leader, so the term number is your proof an election happened.
- Committed data (the escalated call) is not lost, because it was replicated to a majority first.
- Kill a second node and quorum is lost (1 of 3): the panel warns and writes are refused.

## Clean up

Ctrl+C in each node terminal, then `docker compose down`.
