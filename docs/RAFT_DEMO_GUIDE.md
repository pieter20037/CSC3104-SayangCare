# Raft Leader Election Demo

Demonstrates Redis-backed queue ordering with Raft-replicated volunteer assignments, leader election, and a restarted node catching up. Requires Docker Desktop. Run commands from the repository root.

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

This is the full call/volunteer application with a Raft queue, not a separate election-only
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
2. Click **Simulate crisis call**, then claim the case as a volunteer. Redis owns queue ordering; the Raft log advances when the volunteer assignment is committed.
3. In Docker Desktop, select the matching `raft-node-N` container and click **Stop**. Leave the dashboard open on one of the surviving nodes.
4. Watch the panel refresh: the stopped node becomes **DOWN**, the cluster reports `2/3 up`, a survivor becomes **Leader**, and the term increases. Quorum remains available, so the cluster still accepts writes.
5. Claim another case to show the new leader can commit an assignment while Redis continues to own the queue.
6. In Docker Desktop, select the stopped node and click **Start**. It rejoins as a **Follower** and catches up from its persistent volume; it does not take leadership back.

## What to say

- Raft elects one leader per term. Followers that stop hearing the leader's heartbeat start an election, and a candidate needs a majority (2 of 3) of votes.
- Killing the leader raises the term and elects a new leader, so the term number is your proof an election happened.
- Assignment ownership is committed to a majority before the claim succeeds. Redis remains responsible for queue order and pending items.
- Stop a second node in Docker Desktop. With only one of three nodes available, quorum is lost; the panel warns and writes are refused. Start it again before continuing.

## Clean up

Stop the stack while preserving database and Raft data:

```bash
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft down
```

To reset the demo completely, remove the named volumes from Docker Desktop or run `docker compose ... down -v`; removing volumes deletes the database and Raft node state.
