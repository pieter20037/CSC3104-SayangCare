# Professor consultation: SayangCare progress, evidence and decisions

Prepared 26 October 2026. This is a speaking brief, not a claim that the
prototype is production-ready or clinically validated.

## 1. A one-minute explanation you can say

> We built SayangCare, a stateful voice-support prototype in Rust/Actix Web.
> Twilio-facing handlers support conversation and volunteer handoff, and a local
> dashboard lets us test without making paid phone calls. API replicas share
> durable session state, alerts and assignment history. We implemented inference
> circuit breakers, bounded fallback, high-risk queueing, Kubernetes deployment,
> autoscaling and monitoring.
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
| Consensus/quorum | Optional OpenRaft queue: majority commits and leader election | In-process Raft tests and a separate demo path exist; do not claim the default queue uses Raft |
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

## 6. Suggested ten-minute meeting/demo

1. **One minute:** read the opening explanation.
2. **Two minutes:** open dashboard, simulated lifecycle and transcript. Show
   synthetic escalation/queue/claim if comfortable. No real distress data.
3. **Two minutes:** show Grafana/Prometheus and HPA/cluster screenshot.
4. **Two minutes:** explain the Redis loss -> durable checkpoint change, then
   availability/durability trade-off with the results table.
5. **Three minutes:** ask the decisions below and write down answers.

Before leaving home: start/check the cluster, open the report and screenshots,
confirm the dashboard works, and have a screenshot-only fallback. **Do not
launch a 15-minute cooldown experiment or primary deletion during a short
consultation unless invited.** A video/screenshot of an executed test is safer
than a risky last-minute live fault.

The optional Raft demonstration uses a separate Compose topology; it is not the
default Kubernetes queue mode. Consult [Raft demo guide](RAFT_DEMO_GUIDE.md)
and prepare separately; do not launch it alongside timed Kubernetes benchmarks.

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
