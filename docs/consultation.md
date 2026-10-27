# Professor consultation: SayangCare progress, evidence and decisions

Updated 27 October 2026 (Singapore time). This is a speaking brief, not a claim that the
prototype is production-ready or clinically validated.

## 0. Presentation plan and startup checklist

**Recommended story:** show the complete application workflow first, demonstrate
Raft leader election in that same application, then show Kubernetes monitoring
and saved load/failover evidence. Finish with the engineering trade-offs and
specific decisions you need. Use synthetic data only.

### Two deployment modes, one application codebase

| Mode | Start command | Dashboard | What to demonstrate |
| --- | --- | --- | --- |
| Docker Raft application | `.\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser` | http://127.0.0.1:18081/dashboard through port 18083 | Caller lab, escalation, volunteer work, Operations, replicated queue, election and rejoin |
| Kubernetes application | `.\scripts\start-dashboard.ps1 -Mode Kubernetes -OpenBrowser` | http://127.0.0.1:18080/dashboard | API replicas, Redis/Sentinel, CNPG PostgreSQL HA, HPA and Kubernetes monitoring |

The original command `.\scripts\start-dashboard.ps1 -OpenBrowser` still selects
**Kubernetes**, not Raft. Use the explicit Raft command for the integrated
call-and-election presentation.

Both deployments can run at the same time if laptop resources permit, but they
have **separate queues and PostgreSQL deployments**. A call created on port
18082 is shared with the other Docker Raft nodes, not automatically with port
18080. The Raft stack uses one Docker PostgreSQL service; it does not demonstrate
CNPG database HA or Kubernetes HPA. Do not mix the deployments' measurements.

Say:

> These are two deployment configurations of the same modular application.
> I will use the Raft configuration for the end-to-end application and consensus
> demonstration, then the Kubernetes configuration for orchestration and the
> recorded resilience/load experiments. Starting both does not join their data.

### Prepare 30 minutes before the consultation

1. Start Docker Desktop and wait for the engine.
2. Open PowerShell in the repository:

   ```powershell
   Set-Location "C:\Users\chewh\Documents\GitHub\CSC3104-SayangCare"
   .\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
   ```

   If local policy blocks the script, and your machine policy permits it:
   `Set-ExecutionPolicy -Scope Process RemoteSigned`.
   The launcher rebuilds current images/migrations, starts the services, checks
   all three APIs, storage and dashboards, and checks leader/term agreement.
   The readiness timeout starts after the build. Containers remain running
   after this launcher exits. Do the build beforehand, not during the pitch.
3. Open these three browser tabs:
   - http://127.0.0.1:18081/dashboard
   - http://127.0.0.1:18082/dashboard
   - http://127.0.0.1:18083/dashboard
4. Confirm one leader, two followers and `3/3 up`. Note which tab belongs to a
   follower so you can keep it open when stopping the leader.
5. Rehearse a simulated call and volunteer claim. Existing synthetic queue
   entries may remain from previous tests; a Claim next action need not select
   the SID you just created. Verify the selected SID rather than assuming it.
6. If showing Kubernetes live, open a second PowerShell window:

   ```powershell
   .\scripts\start-dashboard.ps1 -Mode Kubernetes -OpenBrowser
   ```

   Keep this terminal open: it owns the port-forward. This mode expects the
   Kubernetes resources to have already been deployed; it is not a clean
   installation command. Keep port 18080 for Kubernetes; do not use 18081 as
   its alternate port while the Raft stack is running.
7. Check the existing Kubernetes deployment:

   ```powershell
   kubectl --context minikube -n sayangcare get pods,hpa
   kubectl --context minikube -n sayangcare get cluster sayangcare-db
   ```

   Confirm the actual Ready counts; do not describe stale screenshots as live.
8. For already installed monitoring, use separate terminals and leave them open:

   ```powershell
   kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
   ```

   ```powershell
   kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
   ```

   Open http://127.0.0.1:13000 and http://127.0.0.1:19090.
   These observe the configured Kubernetes targets, not automatically the
   Docker Raft calls. Obtain credentials privately; do not display secrets.
9. Open the results table in section 5, saved reports, and backup screenshots.
   Avoid running load benchmarks alongside both live deployments: they compete
   for laptop CPU/memory and change the experiment conditions.
10. Do not delete database/Raft volumes, expose `.env`, or perform live primary
    database deletion during the consultation unless explicitly invited.

**Fallback:** if a live service fails, show the error honestly, switch to saved
evidence, and explain what that evidence actually tested. Do not improvise a
database reset or present a static screenshot as live.

## 1. A one-minute explanation you can say

> We built SayangCare, a stateful voice-support prototype in Rust/Actix Web.
> Twilio-facing handlers support conversation and volunteer handoff, and a local
> dashboard lets us test without making paid phone calls. API replicas share
> durable session state, alerts and assignment history. We implemented inference
> circuit breakers, bounded fallback, high-risk queueing, Kubernetes deployment,
> autoscaling and monitoring.
> In our Raft deployment, the same caller and volunteer application uses a
> three-node replicated queue, so we can show a leader failing and a survivor
> continuing to accept escalations.
>
> Our chaos tests found a real flaw: Redis automatic failover lost 17
> acknowledged session creations. We changed the design to commit versioned
> PostgreSQL checkpoints before acknowledging updates, with Redis as a
> best-effort projection. The repeated Redis test preserved all acknowledged
> operations. We also fixed graceful API shutdown and passed a 100-user pod-loss
> test with zero HTTP failures.
>
> Our remaining limits are explicit: PostgreSQL failover preserves data but
> briefly fails requests, and sustained load exposes database pool timeouts.
> HPA scales correctly but does not yet meet all latency/error targets.
> Testing is single-node Minikube; live Twilio testing still needs an agreed
> arrangement. We would like advice on the required availability target,
> final evaluation scope and safe live-call testing.

## 2. What we built, in plain English

| Area | Implemented feature | Why it matters |
| --- | --- | --- |
| Voice ingress | Twilio form/TwiML handling, signed webhook checks, outbound REST/connect handler | Connects the HTTP application to telephone workflows; local contracts are tested, not yet full live voice |
| Developer experience | Call Lab/dashboard and simulated incoming/turn/hangup flow | Repeatable synthetic demonstrations without actual phone calls |
| Session state | Versioned PostgreSQL checkpoints before acknowledgment; best-effort Redis projection | A confirmed session update does not depend only on asynchronously replicated Redis |
| Archival | JSONB transcripts, terminal archive before version-safe cleanup, retries | Keeps completed conversations and avoids deleting a newer update |
| Recovery | Thirty-second sweep, five-minute inactivity abandonment, terminal recovery | Interrupted/failed archival work can finish after restoration |
| AI | Groq sentiment and reply stages with separate breakers and deadlines | Provider slowness/failure does not cause unbounded inference waits |
| Safety boundaries | Deterministic high-risk classification, fixed response, queue/human review | High-risk routing does not depend entirely on generative output; not a validated clinical classifier |
| Queue | Default Redis priority queue; optional OpenRaft backend | Coordinates volunteer work; Raft is an explicitly enabled alternative, not the default |
| Assignments/alerts | PostgreSQL assignment history/bindings and shared operator alerts | Auditable ownership and alerts surviving API replacement |
| Deployment | API replicas, Services/Ingress, Redis/Sentinel, three-instance CNPG, PVCs | Moves state outside API processes and supports pod-level recovery |
| Elasticity | CPU/memory/request-rate HPA | Changes API replica count with measured demand |
| Observability | Prometheus metrics/Adapter, seven Grafana panels, stage timings | Shows throughput/errors/latency, pools, breakers and replica behaviour |
| Reproducibility | k6 lifecycles, fault/HPA runners, native exit capture, exact-ID SQL audits | Tests data preservation and performance, not just HTTP 200 |

The API is a **replicated modular monolith**, not seven independently deployed
microservices. Seven Rust crates separate responsibilities within the service.

### Current architecture

```text
Caller <-> Twilio webhooks <-> API replicas behind Service/Ingress
                                   |
                    +--------------+------------------+
                    |              |                  |
             PostgreSQL         Redis/Sentinel      Groq
             checkpoints        hot projection,     sentiment/reply
             (read/CAS and      default queue,      with breakers,
              commit authority) shared coordination deadlines/fallback
                    |
             terminal archives, alerts, assignment history

Optional queue alternative: three OpenRaft nodes (separate configured mode).
```

## 3. Distributed-systems concepts we can explain

| Concept | Where it appears | Evidence and honest boundary |
| --- | --- | --- |
| Replication | API replicas; Redis replicas; three PostgreSQL instances | Several pods on one machine are not several failure-independent machines |
| Leader discovery/failover | Sentinel discovers/promotes Redis master; CNPG changes DB primary | Redis promotion demonstrated; graceful DB replacement measured; not arbitrary partition testing |
| Consensus/quorum | Optional OpenRaft queue: majority commits and leader election | Docker application rehearsal on 27 October verified leader loss, a new term, another escalation, and restarted-node catch-up; the default Kubernetes queue is still Redis |
| Concurrency control | Monotonic versions and SQL compare-and-swap | Competing stale writes cannot silently overwrite the latest checkpoint |
| Durability | PostgreSQL commit before acknowledgment with synchronous replica requirement | Exact acknowledged operations survive recorded faults; no proof for every possible failure |
| Partial failure | API, Redis, PostgreSQL and inference can fail independently | Fault tests separate service availability from persistence success |
| Fault tolerance | External state, recovery worker, pod replacement and awaited drain | Pod-loss run passes; DB failover still returns errors |
| Eventual recovery | Retained checkpoints eventually archive after recovery | Redis projection may lag; queue/session changes are not one atomic cross-system transaction |
| Idempotence | Stable call IDs/create-if-absent, version checks; queue command IDs | Do not claim arbitrary turn retries are exactly-once; uncertain commit/response outcomes remain possible |
| Circuit breaker | Shared bounded failure/latency state, Closed/Open/Half-Open, recovery probe | Mock outage/recovery measured; not a clinical safety guarantee |
| Backpressure/resource bounds | AI concurrency/deadlines and finite DB pools | Limits exist, but pool waits still cause failures under sustained load |
| Elasticity | HPA CPU, memory and per-pod request rate | API scaling does not automatically scale shared database capacity |
| Observability | Metrics, traces/logs, histograms, timestamps, SQL audits | Scrape timing/quantiles differ from individual caller-turn measurements |

**Important distinctions to say clearly:**

- Redis Sentinel is **not Raft** and asynchronous Redis replication is not a
  zero-loss commit protocol.
- PostgreSQL uses database replication, not the application's OpenRaft engine.
- A PVC is persistent storage, **not an off-cluster backup**.
- Availability = requests succeed now. Durability = committed data survives.
  Our database test demonstrates these can differ.
- We have not proved cross-system linearizability, exactly-once arbitrary calls,
  partition tolerance for every component, or production-level availability.
- A model sentiment signal is not equivalent to deterministic risk level or
  a validated medical assessment.

## 4. What was broken, and what we changed

1. **Redis-first acknowledged writes:** automatic failover lost 17 of 1,597
   acknowledged creates, absent from SQL and all Redis nodes. We introduced
   versioned durable checkpoints before success. Redis is no longer session
   read/CAS authority. This increases database work and storage.
2. **Sentiment provider compatibility:** initial live sample returned three
   replies but zero successful sentiment scores. Updated GPT-OSS request
   settings/JSON output and stricter evidence gates. Ten complete sentiment/
   reply pairs then passed. The old run did not capture enough token detail to
   prove one specific root cause.
3. **API shutdown:** custom signal handling could exit without awaited draining,
   causing a hangup 502. Disabled duplicate signal handling and concurrently
   awaited graceful stop/server completion. Registered in-flight-response test
   and repeated pod loss passed.
4. **Database graceful deletion delay:** operator waited for WAL receivers
   during a long smart shutdown with pooled connections. Minikube timeout
   changed 45 -> 5 seconds; 90-second stop allowance and required synchronous
   replica stayed intact. Recovery improved, not eliminated.
5. **Queue stale claims:** protected claimed items and skipped stale entries
   instead of returning an unnecessary Claim-next 409.
6. **Alerts/archive recovery:** shared alert storage and recoverable terminal/
   escalated archival, rather than pod-local alerts or silent loss.
7. **Observability/test quality:** refreshed breaker metrics at scrape, added
   transcript/ACK-ID SQL audits, bounded fault runners and reliable native
   exit-code capture. Historical failed runs remain in the evidence.

## 5. Numbers to bring (not every number from every historical run)

All below are synthetic workloads on single-node Windows Minikube unless
explicitly called real Groq. Live phone/audio outcomes are not measured.

| Experiment | Latest useful evidence | Conclusion |
| --- | --- | --- |
| Baseline | 100 VUs / 30 s, 1,358 lifecycles, no HTTP failures, p95/p99 997.87/1,155.75 ms, native 0, SQL verified | Passed |
| API pod deletion | 100 VUs / 60 s, 2,090 lifecycles, no HTTP failures, p95/p99 1,497.16/2,197.10 ms, native 0, SQL verified | Passed; Ready replacement 10.112 s is not service downtime |
| Automatic Redis promotion | 20 VUs / 120 s, 2,149 lifecycles, no missing acknowledged operations/HTTP errors | Passed recorded fault after checkpointing |
| PostgreSQL primary replacement | Recovery 60.995 -> 27.943 s; HTTP failures 2.634% -> 1.600%; 1,461 acknowledged create/turn/hangup IDs preserved; native 99 | Improved recovery; zero-error availability fails |
| Corrected-image HPA | 100 VUs / 120 s, 3,421 iterations; replicas 3 -> 5 -> 3; desired increase 71.3 s; additional Ready 89.8 s; cooldown 371.2 s | Scaling demonstrated |
| Same HPA workload acceptance | Seven HTTP failures/13,676 requests (0.0512%), p95/p99 2,200.26/3,706.21 ms, native 99 | Zero-failed-lifecycle and p95 <2 s gates fail |
| HPA data audit | 3,418 acknowledged creates, 3,415 turns, 3,418 hangups; no missing acknowledged data or pending checkpoints | Failed/unacknowledged requests are not invented successful turns |
| Real Groq | Ten sentiment/reply pairs, no fallback, caller median 1.13 s/max 1.44 s | Small live sample, not statistical production tails |
| Automated checks | 100 workspace tests passed, five ignored; PostgreSQL integration separately passed; eight Python evidence tests; native-code 0/99 check | Ignored tests are not successes |
| Docker Raft rehearsal, 27 October | Three healthy APIs; crisis simulation escalated; stopped leader node 1; node 3 elected, term 1 -> 2; another crisis simulation escalated with 2/3 up; node 1 restarted as follower; all applied indexes matched at 1,105 | Interactive functional rehearsal, not a timed load test, abrupt-crash test or independent durability audit |

The failing HPA run's API logs identify **pool acquisition timeouts**. The
three-connection pool respects the projected budget:
`(20 + 1 surge + 1 terminating) * 3 + 16 reserved = 82` against a previously
measured database maximum of 100. Do not suggest blindly enlarging it.
This budget does not prove query/storage throughput capacity.

Percentiles above are HTTP latency unless labelled caller-turn. Different run
durations and versions are not controlled equal-duration performance comparisons.

Evidence:
[dated results](PHASE_5_LOCAL_RESULTS.md),
[current validation and raw links](TEST04_AND_AI_VALIDATION.md),
[final HPA report](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-evaluation.json),
[final HPA SQL audit](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-sql-audit.json).

## 6. Presentation-ready walkthrough (15-20 minutes)

For a ten-minute slot: opening (1 min), application/escalation (2 min), Raft
failover/rejoin (2 min), evidence and trade-offs (2 min), decisions (3 min).
Mention the remaining features using section 2 rather than rushing every click.

### A. Introduce the problem and architecture (1-2 minutes)

Say:

> A voice-support session must survive an API replacement, preserve its
> conversation, and route urgent work to a human. Our focus is the reliability
> of that workflow, not replacing professional assessment with an AI model.
> We separated domain logic and infrastructure into Rust crates, but deploy
> them as a replicated modular monolith.

Show the architecture in section 2. Identify the PostgreSQL acknowledgment
boundary, the selected queue backend, and the optional external AI provider.

### B. Caller lab: an ordinary conversation (2 minutes)

Use a Docker Raft dashboard throughout parts B-E.

1. Select **Caller lab** and generate a fresh Call SID.
2. Click **Start call**.
3. Enter a neutral synthetic statement, such as "I would like someone to talk
   to today", and click **Record turn**.
4. Show the transcript, session status, risk field and increasing version.
5. Click **End call** and refresh the session to show its terminal state.

Say:

> The Call SID identifies the session; each accepted mutation advances a
> version. PostgreSQL checkpoints are committed before success is returned.
> Redis is a best-effort projection rather than the authoritative session store.
> Completed conversations are archived, and recovery workers retry unfinished
> archival work.

The dashboard alone is not proof of durable replication. Refer to the SQL
audits in section 5 for that claim.

**AI distinction:** explain that normal turns can use separate sentiment and
reply stages, with bounded deadlines and fallback. The Docker Raft Compose
configuration does not explicitly pass Groq settings into its API services;
do not assume that putting a Groq key in the host `.env` enables live inference
inside those containers. A deterministic reply or "Not analyzed" is not evidence
of a live model call. Use the recorded ten-pair Groq result when discussing
verified provider integration.

The browser voice button is a local simulation, not proof of a real Twilio
phone call. Microphone/browser support must be rehearsed before showing it.
Browser voice automatically submits finalized recognized speech and resumes
listening after the spoken reply finishes. Recognition starts only after session
creation succeeds. Recognition restarts after silence/end-without-result.
The extra Speak next turn and Stop voice controls have been removed.
**End call** remains available during voice processing/playback, stops microphone
and speech immediately, then ends the session after any submitted turn finishes.
Caller lab, Volunteer desk and Operations tabs remain available during processing
and playback. Leaving Caller lab stops the microphone and spoken reply but keeps
the call session open; a turn already submitted still finishes saving. Start
browser voice again to begin a new voice call.
Recognition can still mishear background audio; this is a browser simulation,
not validated telephone speech recognition.
Overview queue, roster and alert counts refresh every five seconds in every
workspace tab while the page is visible and no foreground operation is busy.
Returning to the browser tab triggers a refresh; failures are shown beside
the overview rather than silently presenting stale counts as current.
The initial escalation uses a controlled safety message. Subsequent turns use
the configured LLM for supportive conversation while preserving escalation;
provider failure uses bounded fallback without clearing escalated state.
Queued or assigned is not proof of connection. Verified connection tracking and
automatic AI cutoff on actual volunteer connection remain separate follow-up work.

Risk-routing regression update: explicit phrases such as "cut myself",
"cutting myself", "suicidal thoughts" and "want to end it" trigger immediate
escalation in the shared JSON/Twilio classifier. Matching normalizes spacing,
punctuation and curly apostrophes. This remains a conservative phrase-based
prototype: it does not reliably understand negation, quoted statements, intent
or every paraphrase. Sentiment scoring is separate and is not the authority
for initiating handoff on its own, except that an explicit model `self_harm`
marker now raises risk to at least 4/5 for human review in both handlers.
Negative valence or distress severity alone does not trigger this rule.
Model false positives remain possible. A missing sentiment score must not prevent these
deterministic high-risk phrases from escalating.

The AI reply policy requests brief spoken responses relevant to the current
message, distinguishes substance-use concerns from expressed suicidal intent,
and avoids unconfirmed country-specific emergency numbers. Sentiment instructions
likewise distinguish drug mentions from explicit self-harm. These are model
instructions, not guaranteed classifications; controlled tests and review remain
necessary. Deterministic escalation responses intentionally remain fixed.

### C. Escalation and volunteer work (2-3 minutes)

1. Click **Simulate crisis call**. Use only its synthetic test data.
2. Show `ESCALATED`, risk `5 / 5`, the fixed response and the session SID.
3. Select **Volunteer desk**, refresh the roster and choose an available
   volunteer under **Working as**.
4. Refresh the queue and click **Claim next**.
5. Show the actual claimed SID in **Assigned cases**. If older work was
   selected, explain the queue order rather than pretending it is the new call.
6. Show the selected case's transcript and ownership. If useful, demonstrate
   the case transfer or resolution controls that are available for that case.
   Rehearse their preconditions beforehand; do not imply every state permits
   every action.

Say:

> Deterministic high-risk routing bypasses generative replies and requests
> human support. Priority queueing coordinates work; relational assignment
> bindings and history record who owns it. Simulated calls do not dial a
> volunteer's phone.

Mention stale-entry handling and assignment recovery. Do not claim the queue
command, session checkpoint and assignment record form a single atomic
transaction across all stores.

### D. Operations and shared state (1 minute)

1. Select **Operations**.
2. Refresh **Operator alerts** and **Request activity**.
3. Explain that request activity is a dashboard diagnostic view, while shared
   operator alerts and assignment history are persisted application records.
4. Open another Raft node's dashboard and inspect the same SID if needed to
   demonstrate that session state is not confined to one API process.

Say:

> Replacing an API process should not erase the durable session or its audit
> history. These workspace tabs are demonstration views, not implemented
> role-based authentication.

### E. Raft leader failure and recovery (2-3 minutes)

1. Show `3/3 up`, one leader and two followers. Note leader ID, term and
   log/applied indexes.
2. Keep a surviving follower's dashboard open.
3. In Docker Desktop, **Stop only the current leader's `raft-node-N` container**.
   Stop is a graceful process stop; label it as such, not an abrupt crash.
4. Wait for the panel to show the unavailable node and a new leader. Point
   out the increased term and `2/3` quorum.
5. Run **Simulate crisis call** again. Show the successful escalation and
   advancing replicated indexes.
6. Start the stopped container. Show it rejoining and catching up. Do not
   promise it can never lead again; future elections can change leadership.
7. Optionally stop a second node while one remains down to show majority
   loss. New Raft queue commits must not succeed with only one voter. A role
   label alone is not proof that writes can commit. Restore every node afterward.

Say:

> A three-voter cluster needs two votes to elect a leader and a majority to
> commit work. Losing one node preserves quorum. Losing two sacrifices write
> availability rather than allowing independent conflicting committed queues.
> The election term and a successful post-election queue mutation are more
> meaningful evidence than simply seeing a green status label.

For an explicitly requested abrupt-crash experiment, replace `raft-node-1`
below with the current leader's service and run from the repository root:

```powershell
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft kill raft-node-1
# After observing the election and a successful new escalation:
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft start raft-node-1
```

Clearly distinguish this from the graceful-stop rehearsal. Do not stop
PostgreSQL or Redis during this test: that would mix independent faults.

**Browser warning:** the cluster panel may display internal Docker names such
as `raft-node-1:8080`. Those are peer-network addresses, not laptop URLs.
Use the prepared localhost tabs on 18081-18083.

### F. Kubernetes, monitoring and elasticity (2 minutes)

Explicitly announce the switch to the **separate Kubernetes deployment**.

Show:

```powershell
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n sayangcare get cluster sayangcare-db
```

Then show the provisioned Grafana dashboard and a saved HPA run.

Say:

> Kubernetes replaces failed API pods and HPA adjusts replica count. Prometheus
> collects request, latency, resource and circuit-breaker signals; Grafana
> visualizes them. Port-forwarding makes the existing service available on
> my laptop; it does not publicly expose it or make the topology multi-node.
> Our recorded HPA run scaled from three to five and back, but still failed
> the strict latency and zero-failed-lifecycle gates.

Do not wait through the recorded 371-second downscale period live. Use the
saved timestamps. If graphs are idle, explain that the Docker demo traffic is
not automatically scraped as Kubernetes traffic.

### G. The strongest engineering story: finding and fixing data loss (2 minutes)

1. Show the original finding: **17 of 1,597 acknowledged session creates lost**
   during Redis asynchronous failover.
2. Show the checkpoint architecture change: SQL version/CAS commit before ACK,
   Redis as projection, recovery from retained durable checkpoints.
3. Show the repeated Redis result: **2,149 lifecycles, no HTTP failures or
   missing acknowledged operations** in the independent audit.
4. Explain the cost: PostgreSQL now receives active-session writes; pool waits
   appear under sustained load.
5. Contrast the API pod-loss pass with the PostgreSQL availability failure.

Say:

> We did not just check whether containers restarted. We recorded successful
> operation IDs and independently checked their durable transcripts. Our design
> change fixed observed data loss, but it increased database pressure. We keep
> the failed availability and load results instead of weakening the thresholds.

### H. AI, telephony and safety boundaries (1 minute)

- Live Groq evidence: ten sentiment/reply pairs succeeded; small sample only.
- Controlled mock outage/recovery: tests fallback and circuit-breaker recovery,
  not provider latency or clinical safety.
- Twilio routes: inbound voice, gathered speech, status callbacks and outbound
  connect; local contract tests are not end-to-end phone/audio validation.
- High-risk fixed routing is a heuristic safety boundary, not a clinical
  assessment. The prototype is not an emergency service.
- Use synthetic callers; do not display `.env`, keys, tokens, real phone
  numbers or sensitive transcripts. Live telephony needs consent and budget.

### I. Close with decisions, not a claim of perfection (2-3 minutes)

> The prototype demonstrates the application workflow, quorum-based queue
> failover and measured Kubernetes recovery. Our strongest result is preserving
> acknowledged data after discovering a real loss mode. The remaining work is
> database availability/capacity, broader failure testing, and approved live
> telephony. We would like your guidance on which of those are required for
> final acceptance and what availability target we should meet.

Ask the highest-priority questions in section 7 and record the answers in
section 9.

### Code tabs to prepare if asked "where is this implemented?"

| File | Show | Explain |
| --- | --- | --- |
| [API assembly](../crates/api/src/main.rs) | Routes and awaited graceful shutdown | One service with multiple interfaces; draining in-flight work |
| [Application wiring](../crates/api/src/state.rs) | Redis/Raft queue backend selection | Same application ports with an explicitly selected implementation |
| [Call handlers](../crates/api/src/handlers/call.rs) | Create, turn and hangup flow | Stateful lifecycle and acknowledgment boundary |
| [Checkpoint store](../crates/session-store/src/checkpoint.rs) and [tiered store](../crates/session-store/src/tiered.rs) | SQL versions, commit and projection/archive flow | Root-cause fix for Redis-first loss |
| [Volunteer handlers](../crates/api/src/handlers/volunteer.rs) and [priority queue crate](../crates/priority-queue) | Claim/ownership and queue implementations | Distributed work coordination, audit and reconciliation limits |
| [Raft handlers](../crates/api/src/handlers/raft.rs) | Peer RPC and read-only cluster status | Consensus traffic versus dashboard observations |
| [Performance module](../crates/api/src/performance.rs) and [circuit-breaker crate](../crates/circuit-breaker) | Deadlines, bounded concurrency and breaker transitions | Dependency fault isolation |
| [Lifecycle evaluator](../tests/k6/evaluate-lifecycle.ps1) | Acknowledged-ID recording and SQL audit | Validate durable outcomes, not only status codes |

Do not read all files line by line. Open the relevant function only when the
professor asks. Further procedure: [Raft demo guide](RAFT_DEMO_GUIDE.md),
[current validation](TEST04_AND_AI_VALIDATION.md).

Report copies:
[Word](../documents/GROUP25_UPDATED.docx),
[PDF](../documents/GROUP25_UPDATED.pdf),
[source](GROUP25.md). These remain drafts pending team/requirements review.

## 7. Questions to ask the professor (highest priority first)

### A. Evaluation and remaining scope

1. **What does successful dependency failover require for this project?**
   Zero failed requests, bounded recovery, zero acknowledged-data loss, or a
   combination? We currently preserve acknowledged data but briefly fail DB
   requests. We will retain existing failures even if the agreed SLO differs.
2. **Is single-node pod-level fault testing sufficient?** If not, which minimal
   multi-node/node-failure scenario is required by the rubric?
3. **Is our evidence-backed checkpoint architecture change appropriate?**
   It fixes observed Redis loss but shifts active-call load to PostgreSQL.
   Is explaining/measuring that trade-off enough, or is a different architecture
   expected?
4. **For TEST-05, is measured scaling plus honest failed performance gates an
   acceptable evaluation result, or must we resolve the capacity failure first?**
5. **Which Phase 6 work is mandatory versus future work?** Pool/capacity tuning,
   checkpoint retention, projection reconciliation, multi-node tests, backup
   restoration, external security and extended AI samples?

### B. Twilio, money, consent and safe operation

6. **What live TEL-06 arrangement is approved?** The team is in Singapore with
   a US Twilio number. Are verified test numbers, a supervised call, or a
   recorded demonstration acceptable? What budget/account restrictions apply?
7. **Which specific live behaviours must be demonstrated?** Inbound speech,
   audio fallback playback, escalation and volunteer outbound dispatch?
8. **What consent/data-retention requirements apply to testers?** We should
   not send real sensitive caller information to inference or use the
   prototype as a crisis service.
9. **Can the ten-turn Groq smoke support provider-integration validation,**
   with mocked inference used for high-concurrency tests to avoid cost/rate
   limits? Is a larger sample required?

### C. Report and demonstration

10. Confirm report template, page limit, required plots/screenshots, proposal
    changes and individual-contribution documentation.
11. Should test IDs be marked "experiment completed, acceptance failed" and
    linked to separate follow-ups? This avoids implying unperformed tests or
    falsely passing failed ones.
12. Is our optional OpenRaft evidence sufficient, or is a separately recorded
    leader-loss/quorum demonstration required for assessment?

## 8. Answers to likely professor questions

**"Why Redis if PostgreSQL now stores active state?"**
Redis remains a best-effort hot projection and supports queue/coordination
workloads. Session reads/CAS currently use PostgreSQL authority. We should not
claim Redis accelerates authoritative session reads in this implementation.
Its value/cost and reconciliation policy are a legitimate design discussion.

**"Does scaling API pods solve the failures?"**
Not automatically. More API replicas can increase pressure on the shared
database. We need measured query/pool/admission capacity, not just more pods.

**"Did you lose successful turns in the latest HPA test?"**
The exact-ID audit found no missing acknowledged turns. Some requests failed
before a successful turn response; those are availability failures, not proven
loss of successfully acknowledged turn data.

**"Is 28 seconds the exact database outage?"**
No. It is sampled changed-primary plus API readiness recovery from fault
injection. Request errors and timestamps separately describe user-visible impact.

**"Does a successful test mean this is ready for real callers?"**
No. Workloads are synthetic; risk scoring is heuristic, live Twilio validation
is deferred, and privacy/safety/production qualification remain outstanding.

## 9. Take away an agreed action list

Write the professor's actual decisions below after the meeting; these are not
already agreed requirements:

- Required availability/recovery/data-loss targets:
- Minimum cluster topology/fault scenarios:
- Approved live Twilio test and budget:
- Mandatory versus future-work Phase 6 tasks:
- Report/demo format and evidence expectations:
- Who owns each follow-up and target date:

Use [Windows testing](PHASE6_Windows.md) or [Mac testing](PHASE6_MAC.md)
afterward. Do not change acceptance thresholds or architecture merely to make
an existing failed run appear to pass.
