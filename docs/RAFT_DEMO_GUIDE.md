# Raft Leader Election Demo

Demonstrates Redis-backed queue ordering with Raft-replicated volunteer assignments, leader election, and a restarted node catching up. Requires Docker Desktop. Run commands from the repository root.

For the current generation-token protocol, safe upgrades, stale-owner examples,
and reproducible comparison, follow [Volunteer ownership fencing](RAFT_FENCING.md).
Existing active legacy Raft assignments require offline reconciliation; do not
delete retained data to bypass the upgrade guard.

## Start the Cluster

Create `.env` from `.env.example` if needed. Keep the PostgreSQL password and Twilio settings in `.env`; do not commit the file. The Twilio values may be placeholders for this local dashboard demo.

On Windows, start the full Raft-enabled application and open its dashboard with one command:

```powershell
.\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
```

The launcher rebuilds the images (including current database migrations), starts PostgreSQL,
Redis/Sentinel and all three API nodes, then verifies each node's health, storage readiness,
dashboard, and agreement on one leader and two followers. Docker Desktop must be running.
The first build may take several minutes; the readiness timeout starts after the build.
Use `-TimeoutSeconds 300` for a slower machine, or `-CheckOnly` to validate startup without
opening a browser. Containers remain running when the launcher exits.

This is the full call/volunteer application with Redis ordering and optional Raft ownership coordination, not a separate election-only
viewer. Use its dashboards for both calls and election demonstrations. The default launcher
mode (`-Mode Kubernetes`, or no mode argument) still opens the existing Minikube deployment,
which uses Redis queues by default. Running both deployments does not connect their queues
or databases. Kubernetes HPA and database-HA demonstrations belong to the Kubernetes
deployment; the Docker Raft stack does not provide those features.

Alternatively, start the infrastructure and all three Raft API nodes directly:

```bash
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --build
```

Docker Desktop will show three API services: `raft-node-1`, `raft-node-2`, and `raft-node-3`. Each has a persistent data volume. Open any node dashboard:

- Node 1: http://localhost:18081/dashboard
- Node 2: http://localhost:18082/dashboard
- Node 3: http://localhost:18083/dashboard

Wait until the Raft cluster panel shows all three nodes up, one leader, two followers, and quorum available.

## Demo steps

1. In the **Raft cluster** panel, note the node labelled **Leader** and its node number.
2. Click **Simulate crisis call**, then claim the case as an on-shift volunteer. Show generation 1. Redis owns queue ordering; the Raft log advances when ownership is committed.
3. In Docker Desktop, select the matching `raft-node-N` container and click **Stop**. Leave the dashboard open on one of the surviving nodes.
4. Watch the panel refresh: the stopped node becomes **DOWN**, the cluster reports `2/3 up`, a survivor becomes **Leader**, and the term increases. Quorum remains available, so the cluster still accepts writes.
5. Transfer the existing case to the other on-shift volunteer. Show generation 2 and a successful response; a completed escalation alone does not demonstrate a Raft write. A note with the old generation 1 must return 409.
6. In Docker Desktop, select the stopped node and click **Start**. Verify applied indexes and ownership generations converge. It normally rejoins as a follower; a later election can change leadership.

## What to say

- Raft elects one leader per term. Followers that stop hearing the leader's heartbeat start an election, and a candidate needs a majority (2 of 3) of votes.
- Killing the leader raises the term and elects a new leader, so the term number is your proof an election happened.
- Assignment ownership is committed to a majority before the claim succeeds. Redis remains responsible for queue order and pending items.
- PostgreSQL enforces the installed generation on protected updates. PostgreSQL-only mode can enforce fencing too; compare coordination cost and failure behaviour honestly.
- Stop a second node in Docker Desktop. With only one of three nodes available, new ownership decisions fail closed with 503. An uncertain request remains pending and can commit after recovery: retry its original request ID, not a replacement command. Restore all stopped nodes before continuing.

## Clean up

Stop the stack while preserving database and Raft data:

```bash
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft down
```

Do not remove database or Raft volumes as part of this demonstration.
