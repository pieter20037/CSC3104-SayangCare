# Local Phase 4 and 5 Results

## Exit-code, shutdown and database recovery follow-up

**Final corrected-image HPA evaluation:** 100 VUs / 120 s; **3,421 iterations**,
native exit **99**, scaling **3 -> 5 -> 3**. First desired increase **71.3 s**,
additional Ready **89.8 s**, return to three **371.2 s** after load. Seven
HTTP failures out of 13,676 requests (**0.0512%**), HTTP p95/p99
**2,200.26/3,706.21 ms**. Six lifecycles failed, with nine failed step/check
events. Both zero-failed-lifecycle and p95 < 2 s thresholds failed.
Logs identified PostgreSQL connection-pool acquisition timeouts. Pool size
stays three to preserve the connection budget; no thresholds were weakened.

The exact-ID SQL audit verified **3,418 acknowledged creates, 3,415 turns and
3,418 hangups** without missing acknowledged data, duplicate rows or pending
checkpoints. Failed turns are not counted as successful transcript writes.
See [final HPA report](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-evaluation.json)
and [SQL audit](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-sql-audit.json).
This completes the rerun and exit-code evidence, **not** the load-capacity fix.

The latest repeatable lifecycle runner captures native exits and independently
audits SQL archives. Its 100-VU / 30 s baseline passed with exit **0**:
**1,358 lifecycles**, zero HTTP failures, HTTP p95/p99 **997.87/1,155.75 ms**.
All 1,358 archives/checkpoints had exact transcripts and none remained pending.

An initial post-checkpoint pod-loss test returned one hangup 502 (real failure,
exit 99). A shutdown bug allowed the main runtime to exit without awaiting
Actix's drain. The corrected application awaits graceful stop concurrently
with the server future, disables duplicate signal handling, and leaves five
seconds of grace beyond the 20-second drain and five-second preStop.
The registered in-flight-request shutdown test passes.

The corrected 100-VU / 60 s pod-loss rerun passed with exit **0**:
**2,090 lifecycles**, zero HTTP failures, p95/p99 **1,497.16/2,197.10 ms**.
Replacement Ready was sampled at **10.112 s**; SQL independently verified all
2,090 exact transcripts, completed archives and checkpoints, with zero pending.
An older terminating image appeared in the report inventory; subsequent live
inspection confirmed every serving pod on the same new image.

CNPG operator evidence showed prolonged WAL-receiver shutdown during primary
deletion. The Minikube-only smart-shutdown interval changed **45 -> 5 s**;
the 90-second stop budget and synchronous durability stayed unchanged.
The repeated PostgreSQL fault recovered in **27.943 s**, versus 60.995 s before;
HTTP failures were **1.600%**, versus 2.634%. All **1,461** acknowledged
creates/turns/hangups survived. Native k6 exit **99**: availability still fails.

The first post-checkpoint timed HPA run observed **3 -> 5 -> 3**:
desired increase **84.8 s**, additional Ready **108.7 s**, downscale **383.1 s**
after load. It completed 3,791 iterations but captured native exit **99**,
five failed HTTP requests and nine failed lifecycle/check steps. HTTP p95/p99
were 1,796.60/4,115.16 ms. This is a measured scaling result, not an all-green
load result. The corrected-image result above is also a load acceptance failure.

Full artifacts, exclusions and acceptance details:
[current validation](./TEST04_AND_AI_VALIDATION.md).
Workspace regression: **100 passed, five ignored**; exit-code tests verify
native 0 and 99. No live telephony or new AI-provider requests were made.

## Latest checkpoint architecture: 26 October 2026, 02:36-02:49 SGT

This section supersedes the earlier Redis-first and failed-sentiment status
below without deleting those historical experiments.

- **Redis automatic promotion:** 20 VUs / 120 s with a bounded 60 s master
  pause. `redis-2` -> `redis-1`, recovery sampled at 8.031 s; 2,149 lifecycles,
  zero HTTP failures, HTTP p95/p99 86.08/232.66 ms. Every acknowledged create,
  turn and hangup survived the independent checkpoint/archive audit.
- **PostgreSQL primary replacement:** 20 VUs / 120 s; recovery 60.995 s;
  1,797 iterations, 94.157% lifecycle success, 2.634% HTTP failures.
  All 1,715 acknowledged creates, 1,700 turns and 1,693 hangups survived.
  k6 exit 99: consistency/recovery passed, strict availability failed.
  A later sweeper audit found all 1,716 committed rows archived (1,698 completed,
  18 abandoned), zero pending checkpoints. Failed requests can have committed
  state despite an interrupted acknowledgment; do not infer loss from counts.
- **Real AI:** ten paced synthetic turns, `openai/gpt-oss-20b`; sentiment 10/10,
  replies 10/10, no audio fallback; caller median 1,126.83 ms, max 1,443.03 ms.
  Mean sentiment/reply stages 570.95/514.43 ms. Small live sample, not
  production tail-latency qualification.
- **Mock inference recovery:** 34 lifecycles, validation passed; caller p95
  158.50 ms, max 371.66 ms. Mock timings are not Groq measurements.
- **Durable 100-VU baseline:** 30 s, 1,160 lifecycles / 4,640 HTTP requests,
  zero HTTP or check failures; HTTP p95/p99 1,287.24/1,619.68 ms. Independent
  SQL found 1,160 distinct completed archives, exact two-turn transcripts,
  version 3, zero pending checkpoints. The standalone harness lost its process
  exit code; do not claim a captured zero. Measured k6 thresholds passed.

The production path now commits PostgreSQL checkpoints before acknowledgment,
uses SQL CAS/read authority, and bounds best-effort Redis projections to 100 ms.
This addresses the observed 17-session loss but adds synchronous database cost.
The earlier 100-VU baseline/HPA/API-loss runs below use the old architecture;
they are not post-change certification. Retained terminal checkpoints need a
retention policy; hot-key metrics need not equal durable active-session counts.

Validation: 99 workspace tests passed, five ignored; one registered PostgreSQL
durability integration test explicitly passed; eight offline AI gate tests
passed. Deployed image:
`sha256:fdf15c0346e4a42a6bbe3c047a1fc4d308f9782da7c845345d86e30767057b69`.
CNPG and API each returned to three Ready instances; old paused Redis returned
PONG. No live Twilio call was made.

Raw artifacts and full acceptance details:
[current TEST-04 / AI results](./TEST04_AND_AI_VALIDATION.md),
[independent SQL audit](../artifacts/phase5-final/durable-baseline-sql-audit.json).

## Update: 26 October 2026 deployment and validation

The observations in the original 23 October report below are historical.
The first API rebuild in this session was deployed from commit `57e1714`, with the
Sentinel startup repair in the working tree. API image ID:
`sha256:5c5ef72b356b76135d55c12a6c92953228d488b9aa6fe047127c858323d27e01`.
All traffic used generated synthetic callers, not live Twilio or Groq.

### Repairs and deployment

- Fixed Sentinel restart failure caused by duplicate persisted `known-replica`
  endpoints. Startup removes exact duplicates or entries matching the monitored
  master, retaining a timestamped original config backup on its PVC when changed.
  The blocked unhealthy ordinal was replaced without deleting its PVC. All
  three Sentinels subsequently returned PONG, agreed on Redis master `redis-2`,
  and reported three usable Sentinels for quorum.
- Rebuilt/loaded the API image, applied the request-rate overlay, and rolled
  out the API. The effective PostgreSQL pool is now 3 per process. With the
  documented maximum 20 replicas, one surge, one terminating allowance, and
  16 reserved connections, the projected requirement is 82 against the primary's
  verified `max_connections=100`. This is a capacity projection, not a stress test.
- Installed Grafana with a generated Kubernetes admin secret; verified health,
  the provisioned seven-panel dashboard, and a successful Prometheus proxy query.
- Verified `/api/v1/health`, `/api/v1/ready`, and `/api/v1/metrics` returned 200.
  Prometheus exposed the new route histograms, session/backlog gauges, named
  breaker states, caller-turn timings, and AI/pool metrics. API scrape target
  count varied as HPA adjusted replicas.
- Four offline Python benchmark-evidence tests passed using the repository
  virtual environment. The preceding regression run passed 93 Rust tests and
  four additional Redis integration tests using disposable Redis.

### Current lifecycle tests

| Experiment | VUs / duration | Lifecycles | HTTP requests | HTTP p95 / p99 | HTTP failures | Lifecycle / archive-read consistency |
| --- | --- | ---: | ---: | --- | --- | --- |
| Baseline | 100 / 30 seconds | 2,142 | 8,568 | 390.04 / 514.97 ms | 0% | 100% / 100% |
| One API pod replaced through NGINX Ingress | 100 / 60 seconds | 3,901 | 15,604 | 492.26 / 896.29 ms | 0% | 100% / 100% |

Both k6 runs exited 0 and verified response fields, matching session IDs,
the exact caller statement, and an assistant turn after hangup. The final
ingress experiment recorded 42,911 successful checks and no failed checks.
Replacement readiness and full desired availability were observed 46.68 seconds
after deletion, with a new pod UID. This is not 46.68 seconds of service outage:
the workload observed no failed HTTP requests.
Durations differ, so these rows are not a controlled equal-duration comparison.
HTTP percentiles describe all HTTP requests, not full caller-turn latency.

Raw evidence (timestamps in filenames may be UTC or local, as recorded):

- [Baseline summary](../artifacts/phase5-final/20261026T001152+0800-baseline.json)
- [Ingress pod-loss summary](../artifacts/phase5-final/20261025T162435Z-ingress-podloss.json)
- [Ingress experiment metadata](../artifacts/phase5-final/20261025T162435Z-metadata.json)

The metadata field named `primary_session_audit` contains only a
`pg_stat_activity` connection aggregate, not a session-table/transcript audit.
It does not independently prove archived row counts or transcript integrity.
The consistency result above comes from the k6 post-hangup API reads; an
independent SQL audit was not performed for that particular run. The later
dependency experiments below do include session-table audits.

An earlier attempt mistakenly used a pod-bound API service port-forward and
experienced connection failures. It is retained as invalid-path evidence, not
a passing ingress chaos test. A later preflight was aborted before fault
injection; local HTTP probes must bypass the workstation proxy.
Only the final explicitly ingress-routed experiment above is used for TEST-02.
For legacy k6 summaries, threshold booleans indicate a threshold failure:
`false` here means no failure; interpret together with exit code and values.

### Follow-up: timed HPA, dependency recovery, and shared alerts

All results below were collected on the same single-node test cluster.

**TEST-05: timed HPA evaluation.** A 100-VU, two-minute ingress workload
completed 8,418 lifecycles / 33,672 HTTP requests, with 100% lifecycle and
archive-read checks and no HTTP failures. HTTP p95/p99 were 316.64/610.59 ms.
The first increase in desired replicas was sampled after 67.0 seconds;
the first additional Ready pod after 79.6 seconds. Maximum desired replicas
during load were 9; maximum Ready replicas sampled during load were 7.
The deployment returned to its initial three replicas 493.9 seconds after load.
These are sampled observations, not instantaneous controller timings.

- [Timed HPA report](../artifacts/phase5-final/hpa/20261025T163329Z-evaluation.json)
- [HPA samples](../artifacts/phase5-final/hpa/20261025T163329Z-hpa-samples.csv)

That original report has a null `k6_exit_code`: process-handle retention in the
runner was subsequently fixed. Its summary records passing thresholds and all
checks; the null must not be described as a captured exit code of zero.
A separate five-second harness smoke test captured exit code 0 and correctly
reported neither scale-up nor downscale. k6 request names are now normalized
to avoid one time series per session URL; the smoke stderr was empty.

**TEST-04: PostgreSQL primary pod replacement.** With 30 VUs / 60 seconds,
the primary changed from `sayangcare-db-2` to `sayangcare-db-1`, and changed-primary
plus API readiness was observed after 63.472 seconds. The run completed
1,582 iterations. The independent SQL aggregate found 1,582 unique terminal
rows, all with exactly two expected transcript turns and version 3; no matching
hot keys remained.

This is a data-recovery result, **not a zero-error availability pass**:
k6 exited 99 because `call_lifecycle_failures == 0` was breached.
Lifecycle success was 99.178%; failed HTTP samples were 0.614%.
HTTP p95/p99 were 96.54/211.90 ms. The SQL check was an aggregate, not an
identity-by-identity acknowledgment audit. A graceful primary pod deletion also
includes shutdown/controller time; it is not equivalent to an abrupt machine loss.

- [PostgreSQL report](../artifacts/phase5-final/dependencies/DEP-postgres-20261025T164608Z-report.json)

**Redis master pod replacement, limited result.** Deleting `redis-2` produced
a temporary outage, but the same master returned before promotion was observed.
The initial harness timed out expecting a different master. Retained k6 evidence
has 1,107 successful incoming/turn/hangup/archive lifecycles and 623 failed
incoming attempts. A follow-up primary SQL query found 1,107 unique rows with
two transcript turns. This does not establish automatic promotion or zero
acknowledged-write loss; the failed availability result is retained.
Sentinel logs show subjective/objective down detection, a failover delay, and
the original master returning.

- [Redis replacement report](../artifacts/phase5-final/dependencies/DEP-redis-20261025T165254Z-report.json)
- [Redis postcheck and Sentinel excerpts](../artifacts/phase5-final/dependencies/redis-postcheck.log)

**Redis controlled Sentinel promotion, passed.** A separate 30-VU / 60-second
experiment explicitly requested `SENTINEL FAILOVER`; it did not repeat pod
deletion. Master changed `redis-2` -> `redis-0`; changed-primary plus API readiness
was sampled after 3.153 seconds. All 1,697 lifecycles passed, with zero failed
HTTP requests, p95/p99 78.68/138.13 ms. The harness recorded every acknowledged
incoming session ID and matched all 1,697 to primary PostgreSQL rows.
There were no invalid transcripts or remaining hot keys. This proves controlled
promotion/reconnection under this workload, **not automatic failover during a
sustained master outage**.

- [Controlled Redis promotion report](../artifacts/phase5-final/dependencies/DEP-redis-20261025T170058Z-report.json)

**FIX-03: shared-alert durability, passed.** A supported synthetic high-risk
turn created an escalation alert. Two distinct API pods returned its same
nonempty alert ID. After deleting the originating API pod, a new Ready pod with
a different UID and the surviving pod still returned that ID. The primary
database contained exactly one escalation row both before and after replacement.
The synthetic call was ended and owned port-forwards were stopped.

- [Cross-pod alert evidence](../artifacts/phase5-final/alerts/SIM-ALERT-20261025T165912Z.json)

The alert helper now requires an expected session; empty alert lists cannot
produce a passing result. New dependency and alert runners support Windows
PowerShell 5, `-WhatIf`, unique synthetic IDs, evidence output, and cleanup.
Dependency reports distinguish archival, availability, and primary promotion;
they do not turn a failed k6 run into a passing reliability result.

**TEL-04 local contract validation.** Six telephony tests pass, including
local HTTP-stub tests for Twilio's `/Accounts/{sid}/Calls.json` path,
Basic Auth, URL-encoded To/From/Url/Method fields, non-2xx errors, malformed or
empty Call SIDs, and missing configuration. REST requests now have a ten-second
timeout, and response-body read failures are explicit. These tests use no live
Twilio credentials and place no phone calls. Real phone validation remains TEL-06.

The full workspace subsequently passed **97 Rust tests**, with four Redis tests
still ignored by the default suite (the separate Redis integration run is
recorded above). Four offline Python tests passed. The global formatting check
still reports pre-existing differences in untouched API files; those were not
reformatted as part of this work.

The final API image, including bounded Twilio REST handling, was built, loaded,
and rolled out successfully:
`sha256:40e1ddf10f7f4fb903b72947171d3148c256bb257483e9d751b4739d1f3d27e8`.
Final checks found API 3/3, Redis 3/3, Sentinel 3/3, CNPG 3/3, and Grafana,
Prometheus, and Adapter ready. A two-VU / three-second ingress smoke exited 0.
Raw k6 point samples confirm normalized `url`/`name` labels, including
`POST /api/v1/calls/:id/turn`, rather than session-specific URLs.

- [Final deployment snapshot](../artifacts/phase5-final/final-deployment.json)
- [Final smoke and metric-tag samples](../artifacts/phase5-final/final-smoke.json)

### Still outstanding

- Resolve the acknowledged-session loss exposed by the automatic Redis test
  described below before treating TEST-04 as passed.
- Improve availability during PostgreSQL primary replacement; the strict
  zero-failed-lifecycles threshold did not pass.
- Real-provider latency and live phone/audio validation, pending approval.
- Multi-node HA, long-duration capacity, and backup/restore qualification.

### Latest TEST-04 follow-up and AI configuration

Subsequent live follow-up: private credentials were supplied and three
`openai/gpt-oss-20b` turns were measured at 975.99, 1,168.73, and 1,312.42 ms.
All three replies succeeded, but sentiment had two no-assistant-text errors
and one timeout; overall validation failed. The initial missing-key observations
below are historical, not the current live-test status.
See [the live result](../artifacts/phase5-final/ai/20261025T181738Z-Real/real-ai.json)
and [failure analysis](./TEST04_AND_AI_VALIDATION.md).

The remaining scenarios were exercised, but the resulting acceptance status is
not all green. The complete procedure and proposed durability solution are in
[TEST-04 and AI validation](./TEST04_AND_AI_VALIDATION.md).

- PostgreSQL replacement with 20 VUs / 120 seconds: all **1,858 acknowledged IDs**
  matched primary PostgreSQL records, zero invalid transcripts or matching hot
  keys, promotion/readiness sampled at **65.963 seconds**. Recovery passed with
  explicit `-AllowTransientFailures`; strict k6 availability still failed.
- Redis automatic promotion during a bounded 60-second response pause:
  `redis-0` -> `redis-2` at **37.989 seconds**, but **17 acknowledged incoming
  session IDs** were missing from SQL and later all three Redis nodes.
  This is a failed consistency result, not merely an incomplete experiment.
- Isolated mock inference outage/restoration: **34 archived lifecycles**,
  **30 audio fallbacks**, and measured reply-breaker states **0 -> 1 -> 2 -> 0**.
  Caller-turn p95 **112.81 ms**, p99/max **360.79 ms**. This is not real Groq latency.
- Fixed metrics-scrape freshness after observing that the 30-second background
  gauge missed short Half-Open probes. Scrapes now refresh within the configured
  coordination timeout, returning 503 rather than stale success on timeout.
- Real AI preflight found no exact `SAYANGCARE__AI__API_KEY` in `.env`, the current
  process, or the deployed Secret. It correctly exits before network activity.
  The secure key-entry procedure and runnable live smoke are documented; no
  real-provider timing is claimed.

## Original 23 October 2026 report

**Run date:** 2026-10-23  
**Scope:** Local, single-node Minikube test cluster; generated synthetic callers
only. These results are not production or multi-node availability evidence.

## Test environment

| Component | Version / configuration |
| --- | --- |
| Docker Desktop engine | 29.8.1 |
| Minikube | 1.39.0, Docker driver |
| Kubernetes | 1.37.0, one node |
| Minikube resources | 6 CPUs, 8 GiB memory |
| API image | `sayangcare-api:dev`, `sha256:b6c223c9f08ace7307c6d77236086171ed80e866a43caee47861fd7410f9fa39` |
| Helm | 4.3.1 |
| Prometheus Adapter | APIService available; custom request-rate metrics returned for API pods |
| Grafana | 12.3.1, local ephemeral dashboard |
| k6 | 2.2.0 |

The API, Redis, Redis Sentinel, and PostgreSQL services were ready. The
request-rate HPA started with three API replicas and scaled up to five during
the load checks. The baseline and chaos tests used the local Ingress
port-forward, 100 VUs, 30 seconds, a one-second think time per VU, and unique
synthetic call IDs. Each iteration performed incoming, turn, and hangup.

## k6 results

| Test | Completed lifecycles | HTTP requests | Throughput (requests/s) | p50 HTTP latency | p95 | p99 | HTTP failures |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| Baseline, 100 VUs | 2,898 | 8,694 | 280.19 | 11.60 ms | 69.95 ms | 131.70 ms | 0 (0%) |
| API pod deleted during load, 100 VUs | 2,881 | 8,643 | 278.26 | 12.20 ms | 69.51 ms | 124.01 ms | 0 (0%) |

All HTTP status checks in these earlier runs returned success. The earlier
k6 script did not validate response bodies or archived transcript contents;
repeat with the current
[queue and load-testing guide](./QUEUE_AND_LOAD_TESTING.md) before treating
these as session-consistency evidence. The API pod replacement took about
7.7 seconds during the chaos run. The baseline and chaos k6 summaries are
preserved in [phase5-local-baseline.json](./phase5-local-baseline.json) and
[phase5-local-api-failover.json](./phase5-local-api-failover.json).

These numbers describe this local single-node test setup only. Completed call
lifecycles were measured separately from HTTP requests; HTTP request throughput
must not be reported as call throughput.

## Phase 4 recovery checks

- **API pod recovery:** Deleted one SayangCare API pod during the 100-VU run.
  The Deployment replaced it in approximately 7.7 seconds. API health and
  readiness remained HTTP 200; readiness reported PostgreSQL and Redis `ok`.
- **PostgreSQL primary failover:** Deleted primary `sayangcare-db-1`.
  CloudNativePG promoted `sayangcare-db-2` and restored three ready instances
  in approximately 62.7 seconds. A completed synthetic session and its two
  transcript turns were present on both the old and promoted primary.
- **Redis Sentinel failover:** Deleted the Redis master `redis-0` after Redis
  confirmed replication to two replicas. Sentinel promoted `redis-2`; the
  removed pod recovered in approximately 7.6 seconds. The active synthetic
  session remained readable after promotion and API readiness returned healthy.
- **Monitoring and dashboard:** Prometheus returned SayangCare request-counter
  series, the custom metrics API returned per-pod request-rate values, the HPA
  consumed that metric, and the provisioned three-panel Grafana dashboard and
  Prometheus datasource both loaded successfully.

## Limitations

- Minikube has one Kubernetes node. Pod replacement does not test node, zone, or
  external-site failure.
- These archived benchmark summaries do not include the current script's
  response-body and post-archive transcript checks. HPA replica totals were
  observed, but scale-up/down delay and latency/error changes attributed to
  each replica phase were not measured. Use the
  [HPA evaluation runner](../tests/k6/evaluate-hpa.ps1) for new timed results.
- Prometheus retention is one hour and is not persisted. Grafana is configured
  without persistent storage for local validation.
- The checked-in Grafana chart is deprecated upstream; this local setup is not
  a production deployment recommendation.
- The tests use simulated API calls only; they do not validate Twilio,
  microphone audio, live inference-provider credentials, or public DNS/TLS.
- The load test is a 30-second local benchmark, not a long-duration capacity
  qualification.
