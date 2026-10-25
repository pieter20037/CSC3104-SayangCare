# TEST-04 and real AI validation: results and repeatable procedure

Updated: 26 October 2026, Singapore time.

## Outcome

### Latest exit-code and availability follow-up

**Final corrected-image HPA rerun:** 100 VUs / 120 s, **3,421 iterations**,
native k6 exit **99**. Scaling **3 -> 5 -> 3** was observed: first desired
increase **71.3 s**, first additional Ready **89.8 s**, downscale **371.2 s**
after load. Seven of 13,676 HTTP requests failed (**0.0512%**);
HTTP p95/p99 **2,200.26/3,706.21 ms**. The zero-failed-lifecycle and p95 < 2 s
thresholds failed; the HTTP < 1% threshold passed. Nine failed step/check
events affected six lifecycles, not nine distinct calls.

API logs identified **PostgreSQL pool acquisition timeouts** for failing creates
and turns. The per-process pool remains three to preserve the documented
20-replica connection budget. Raising it blindly would exceed database capacity;
no larger pool, longer deadline or weaker threshold was applied to force a pass.
Independent SQL/ACK auditing found **3,418 acknowledged creates, 3,415 turns
and 3,418 hangups**, zero missing durable creates/valid acknowledged turns/
archives, and zero pending checkpoints. Three archived calls have no successful
turn acknowledgment; they must not be described as lost successful turns.
Availability/capacity refinement remains required.

[Final HPA report](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-evaluation.json)
and [exact-ID SQL audit](../artifacts/phase5-final/hpa-checkpoint-fixed/20261025T195347Z-sql-audit.json).
Final serving image ID:
`sha256:3858e2806bd6e6fcdc51df50eea096cbd3ab064cc19c225ee8e63dd45a79270a`.
The native k6 exit in the JSON is authoritative, not a supervising shell's exit.

The runners now launch k6 through their own .NET `Process` instance, retaining
the actual native exit code while streaming separate stdout/stderr logs.
Regression checks prove both exit 0 and 99. Null historical codes are not
retrospectively filled in.

- **New 100-VU / 30 s baseline:** 1,358 lifecycles, exit **0**, no HTTP failures;
  HTTP p95/p99 997.87/1,155.75 ms; independent SQL confirms 1,358 completed
  archives/checkpoints, exact transcripts, no pending checkpoints.
- **Post-checkpoint API pod deletion:** initially one hangup returned 502 and
  k6 exited 99. Inspection found that the custom signal path did not await
  server draining. The application now disables duplicate Actix signal
  handling, concurrently polls server completion and graceful stop, and allows
  20 seconds to drain within Kubernetes' 30-second grace (including preStop).
  A registered integration-style test proves an in-flight response completes
  during shutdown. The corrected rerun passed: **2,090 lifecycles, exit 0,
  zero HTTP failures**, HTTP p95/p99 1,497.16/2,197.10 ms, 10.112 s replacement
  readiness, all 2,090 archives/transcripts/checkpoints verified by SQL.
- **PostgreSQL graceful primary deletion:** operator logs showed about 40 s
  waiting for WAL receivers while the 45 s smart shutdown allowed pooled
  connections to remain open. The Minikube overlay now uses 5 s smart shutdown,
  retaining the 90 s stop/WAL allowance and required synchronous replication.
  Recovery improved from **60.995 to 27.943 s**. HTTP failures fell from
  **2.634% to 1.600%**, but k6 still exited **99**. Every one of the 1,461
  acknowledged creates/turns/hangups survived. This is an improvement, **not
  zero-error availability**; no mutating-request retry or weaker durability
  was introduced.
- **First post-checkpoint HPA run:** desired/Ready replicas 3 -> 5 -> 3;
  first desired increase 84.8 s, additional Ready 108.7 s, downscale 383.1 s
  after load. Native exit **99** was captured: 3,791 iterations, five HTTP
  failures, nine failed lifecycle/check steps. HTTP failure rate 0.033%;
  HTTP p95/p99 1,796.60/4,115.16 ms. Scaling was observed, but the zero-failed-
  lifecycle gate failed. The corrected-image HPA rerun above is also retained
  as a failed load acceptance result, not relabelled as passing.

Evidence:
[baseline](../artifacts/phase5-final/lifecycle-checkpoint/LIFE-baseline-20261025T192709Z-report.json),
[pod loss after fix](../artifacts/phase5-final/lifecycle-checkpoint/LIFE-pod-failover-20261025T194414Z-report.json),
[PostgreSQL](../artifacts/phase5-final/dependencies/DEP-postgres-20261025T194634Z-report.json),
[first HPA rerun](../artifacts/phase5-final/hpa-checkpoint/20261025T191715Z-evaluation.json).

Earlier lifecycle attempts with occupied port 18186 and a Pending-pod status
parsing bug were excluded harness failures. The later single-502 run was an
actual availability failure and remains retained. The passing pod report's
image inventory included an old terminating pod; follow-up verification found
all three Ready, non-terminating pods on one image. Future reports inventory
only serving pods. The new native-code regression and the registered shutdown
test passed; the workspace now passes **100 tests**, with five ignored.

**Current: acknowledged-operation recovery passes; strict PostgreSQL failover
availability still fails.** The earlier Redis loss and failed sentiment smoke
remain below as historical evidence, not the current deployed result.

### Verified fixes and reruns, 26 October, approximately 02:36-02:49 SGT

Production now commits each versioned session mutation to PostgreSQL before
acknowledgment. Redis is a bounded (100 ms), best-effort hot projection, not
the read/CAS authority. Recovery scans unarchived durable checkpoints, including
sessions missing from Redis. Completed checkpoints are retained alongside the
cold archive to prevent a stale Redis snapshot becoming authoritative again.

| Experiment | Latest observation | Acceptance |
| --- | --- | --- |
| Automatic Redis promotion, 20 VUs / 120 s, 60 s master pause | `redis-2` -> `redis-1`, sampled recovery 8.031 s; 2,149 complete lifecycles; zero HTTP failures; all 2,149 acknowledged creates/turns/hangups durable and archived; no invalid transcripts | Recovery and k6 thresholds passed |
| PostgreSQL primary replacement, 20 VUs / 120 s | Primary changed after 60.995 s; 1,797 iterations, 1,715 acknowledged creates, 1,700 turns and 1,693 hangups; no missing durable creates, invalid acknowledged turns, or missing acknowledged archives | Recovery passed; availability failed: 94.157% lifecycle success, 2.634% HTTP failures, k6 exit 99 |
| Real Groq, `openai/gpt-oss-20b`, 10 sequential synthetic turns | Sentiment 10/10 and replies 10/10; zero audio fallbacks; median 1,126.83 ms, maximum 1,443.03 ms; mean sentiment/reply 570.95/514.43 ms | Strict live gate passed; small-sample smoke, not production p95/p99 |
| Isolated mock inference outage/restoration, 2 workers | 34 lifecycles; median 40.22 ms, p95 158.50 ms, max 371.66 ms; recovery validation passed | Passed; not provider latency |
| New durable 100-VU / 30 s baseline | 1,160 lifecycles, zero failed checks/HTTP requests; HTTP p95/p99 1,287.24/1,619.68 ms; independent SQL confirms 1,160 distinct completed archives, exact two turns, version 3, no pending checkpoints | Measured thresholds/checks pass; standalone harness did not retain the process exit code |

Evidence:
[Redis](../artifacts/phase5-final/dependencies/DEP-redis-20261025T183610Z-report.json),
[PostgreSQL](../artifacts/phase5-final/dependencies/DEP-postgres-20261025T183955Z-report.json),
[live AI](../artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json),
[mock recovery](../artifacts/phase5-final/ai/20261025T184640Z-MockRecovery/breaker-recovery.json),
[durable baseline](../artifacts/k6/unique-DURABLE-20261026-024836908-72289c83-summary.json),
[independent SQL audit](../artifacts/phase5-final/durable-baseline-sql-audit.json).

The subsequent sweeper audit found all 1,716 PostgreSQL-fault checkpoints in the
cold archive: 1,698 completed and 18 abandoned, with zero pending checkpoints.
Sixteen abandoned calls had zero turns; unsuccessful requests are not invented
as successful turns. A committed operation whose acknowledgment was interrupted
can explain an extra durable row; only the exact acknowledged IDs are mandatory.

The trade-off is additional synchronous database work and PostgreSQL availability
on the active-call path. Relative to the earlier 100-VU run (2,142 lifecycles,
390.04 ms HTTP p95), the new run was slower (1,160, 1,287.24 ms). These are
single runs, not a controlled capacity study. Do not tune away the conservative
pool limit or claim zero-downtime PostgreSQL failover. Terminal checkpoint
retention currently has no automatic purge policy; storage/retention planning
and projection reconciliation remain operational work. Redis metrics count
hot projections, not an authoritative durable-session inventory.

GPT-OSS requests now use low reasoning effort, hidden reasoning, a 512-token
output budget and JSON sentiment output. Other models omit the reasoning
options. Token exhaustion and nonfinite sentiment are errors. The previous
failure's exact token consumption was not captured, so no single root cause
is claimed from that run.

Validation: 99 workspace tests passed, five environment-dependent tests ignored;
the registered PostgreSQL durability/concurrent-CAS integration test was then
explicitly executed and passed (one test, not zero). Eight offline AI gate tests
passed. Deployed image:
`sha256:fdf15c0346e4a42a6bbe3c047a1fc4d308f9782da7c845345d86e30767057b69`.

## Historical failed results (superseded, retained)

## Live Groq follow-up: 26 October, approximately 02:17 SGT

The replacement credential was available in ignored local configuration.
A real-looking credential was also found in the tracked example template;
its exact AI-key assignment was cleared without printing or changing the
private key. Never commit a real credential in the example template.

The three-turn smoke used `openai/gpt-oss-20b` at `api.groq.com`:

| Measurement | Observed result |
| --- | --- |
| Sequential synthetic caller turns | 3 |
| Caller-turn durations | 975.99, 1,168.73, 1,312.42 ms |
| Median caller-turn duration | 1,168.73 ms |
| Sample maximum | 1,312.42 ms |
| Mean reply-stage duration | 405.56 ms |
| Reply operations | 3 successful |
| Sentiment operations | 0 successful, 2 errors, 1 timeout |
| Audio fallbacks | 0 |
| Overall validation | Failed, `validation_passed=false` |

The two sentiment errors were classified as no assistant text, not successful
sentiment JSON. The timeout was bounded by the configured sentiment deadline.
The application continued using deterministic risk severity and generated
successful live replies. Zero audio fallbacks therefore does not mean all AI
operations succeeded. Three samples are insufficient for statistical p95/p99
claims; the exported p95/p99 merely equal this sample's maximum.

Evidence:
[live result](../artifacts/phase5-final/ai/20261025T181738Z-Real/real-ai.json)
and [stage metrics](../artifacts/phase5-final/ai/20261025T181738Z-Real/real-ai.prom).

Next: investigate sentiment compatibility and token/deadline budgets for this
reasoning model, retaining the existing failed run. Provider reasoning-token
behavior is a hypothesis, not a confirmed root cause. Do not increase limits,
change models, or claim a passing result without a controlled rerun and measured
successful sentiment operations. No additional provider calls were made after
this three-turn experiment.

| Scenario | Evidence | Acceptance |
| --- | --- | --- |
| PostgreSQL primary replacement, 20 VUs / 120 seconds | 1,858 acknowledged IDs all matched archived rows; no matching hot keys remained; changed primary/readiness after 65.963 seconds | Recovery criteria passed. Strict k6 availability failed (exit 99): 96.868% lifecycle success and 0.854% HTTP failures. |
| Redis automatic promotion during a 60-second response pause, 20 VUs / 120 seconds | Sentinel changed `redis-0` -> `redis-2` after 37.989 seconds, without a manual promotion request | Failed consistency and availability: 17 of 1,597 acknowledged IDs were absent from the cold archive and subsequently all three Redis nodes. |
| Inference failure and restoration, isolated local mock, two workers | 34 complete archived lifecycles; 30 audio fallbacks during the outage; observed Closed -> Open -> Half-Open -> Closed | Passed. Caller-turn p95 112.81 ms, p99/max 360.79 ms. These are mock-provider timings, not Groq latency. |
| Real Groq latency | Three-turn live run above; successful replies but failed sentiment operations | Partial measurement recorded; overall validation failed. Earlier missing-key preflight remains historical evidence. |

Raw evidence:

- [PostgreSQL exact-ID report](../artifacts/phase5-final/dependencies/DEP-postgres-20261025T173620Z-report.json)
- [Automatic Redis report](../artifacts/phase5-final/dependencies/DEP-redis-20261025T172859Z-report.json)
- [Redis missing-ID existence checks](../artifacts/phase5-final/dependencies/DEP-redis-20261025T172859Z-missing-session-existence.csv)
- [Sentinel automatic-promotion log](../artifacts/phase5-final/dependencies/automatic-redis-sentinel.log)
- [Inference recovery result](../artifacts/phase5-final/ai/20261025T174819Z-MockRecovery/breaker-recovery.json)
- [Measured breaker transitions](../artifacts/phase5-final/ai/20261025T174819Z-MockRecovery/breaker-recovery-transitions.json)

The Redis follow-up checked the exact missing IDs, not merely aggregate counts.
Its SQL lookup returned no rows for those IDs. Redis existence checks returned
false on the current master, former master, and third node. Seventeen turn and
hangup requests also failed. The 17 lost records are acknowledged **incoming
session creations**, not proof that 17 successful caller turns were lost.
The experiment must not be described as zero acknowledged-write loss.

Final validation passed the Rust workspace suite and seven offline Python
evidence-gate tests. Pylance reported no syntax errors in the changed Python
files; the PowerShell runners passed AST parsing. The updated API was rebuilt,
loaded, and successfully rolled out to all three replicas. Kubernetes reports
image ID `sha256:27cccbe728521f1fc92ec6e58946d1e41f061d635e28a77bdd2b6935185e64c9`.
Final live ingress checks returned HTTP 200 for health, readiness, and metrics;
PostgreSQL, Redis, and Sentinel each had three ready instances. Test-owned
forwards were stopped. No live provider request was made.

The PostgreSQL recovery test explicitly used `-AllowTransientFailures`. This
does not edit or relax k6 thresholds: its exit 99 and failed availability flag
remain in the report. It only allows the runner to accept separately measured
recovery with intact acknowledged records.

## What was fixed

- Added a bounded `master-pause` fault. `CLIENT PAUSE ... ALL` expires
  automatically and exercises Sentinel detection/election without deleting a
  PVC or manually invoking promotion. It is a Redis response outage, not a
  network partition or machine crash.
- Added an isolated AI runner using owned localhost forwards and unique breaker
  keys. It never modifies the deployed AI Secret or production API settings.
- Fixed stale breaker observability: the background gauge refreshed only every
  30 seconds and missed short probes. The API now refreshes breaker state on
  metrics scrape, within the configured coordination timeout. A timeout returns
  HTTP 503 instead of a success-shaped stale snapshot; Prometheus can record the
  failed scrape during a backing-store outage.
- Added a real-provider preflight that requires the exact
  `SAYANGCARE__AI__API_KEY` name. The live runner only sends credentials to
  `https://api.groq.com`, does not substitute mock credentials, and requires
  successful sentiment and reply stage evidence before passing.
- Added offline tests for missing keys, process-over-file precedence, and
  rejection of unapproved credential destinations. Local benchmark requests
  bypass the workstation HTTP proxy.

## Historical Redis durability diagnosis

**Observed:** automatic promotion works, but acknowledged sessions went missing.
**Not proven:** the precise interleaving responsible for each missing record.
The current code acknowledges hot writes from the master without waiting for
replica acknowledgment. Redis Sentinel with asynchronous replication does not
provide a zero-loss commit protocol or consensus for session writes.

Do not solve this by returning HTTP 200 on failed storage, retrying all mutating
requests blindly, weakening the archive audit, or disabling the fault.

For a zero-acknowledged-loss requirement, the architectural solution is a
durable commit/replay path for acknowledged session mutations:

1. Persist versioned, idempotent session events/checkpoints in the synchronous
   PostgreSQL tier before acknowledging the mutation.
2. Retain CAS/version ordering and stable operation IDs so uncertain outcomes
   can be retried without duplicate turns.
3. Treat Redis as a recoverable hot projection; on missing state, rebuild from
   committed durable data rather than relying only on completed-call archives.
4. Measure the added caller-turn latency and database capacity costs.
5. Repeat the same automatic-promotion test and require every acknowledged ID
   and successful turn to survive, with zero duplicate operations.

The checkpoint implementation above now replaces this historical hot-first
architecture in production. It preserves versioned CAS and does not blindly
retry mutating requests; request-id deduplication for arbitrary turn retries is
not a new guarantee. The database integration test verifies competing updates,
stale writes, unavailable Redis, archive fencing, and failed database commits.

Redis replica `WAIT` acknowledgments and `min-replicas-to-write`/lag fencing can
reduce risk, but must not be presented as equivalent to a durable consensus
commit. Their outage latency, uncertain outcomes, and failover limits also need
measurement.

## Repeat TEST-04

Use a healthy disposable/test Minikube cluster only. These are intentional
faults and can interrupt the local dashboard. No PVC is deleted.

```powershell
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres `
  -Duration 120s -Vus 20 -AllowTransientFailures

.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis `
  -FaultMode master-pause -PauseMilliseconds 60000 `
  -Duration 120s -Vus 20 -AllowTransientFailures

cargo build -p sayangcare-api --locked
.\scripts\run-ai-evaluation.ps1 -Mode MockRecovery
```

Use `-WhatIf` on the dependency runner to inspect its intended fault first.
Use `-KubectlPath` if needed to select the actual executable instead of a
package-manager shim. The AI wrapper owns localhost ports 18190 (Redis),
18191 (PostgreSQL), and 18192 (isolated API), and removes its forwards afterward.
Do not run this concurrently with another benchmark using Redis DBs 10..15.
No database is flushed.

## Configure and run real AI latency safely

The repository already supports Groq. A phone number or Twilio approval is not
needed for this simulated JSON-call test.

1. Obtain a test Groq API key and choose a model available to that account.
2. Do not paste the key into chat, tracked files, screenshots, or a shell command
   containing a literal key. Prefer temporary process environment configuration.
3. From the repository root, run the following PowerShell block. It prompts
   securely and restores prior values even if the test fails:

```powershell
cargo build -p sayangcare-api --locked

$oldKey = $env:SAYANGCARE__AI__API_KEY
$oldModel = $env:SAYANGCARE__AI__MODEL
$oldEndpoint = $env:SAYANGCARE__AI__BASE_URL
$secureKey = Read-Host 'Groq test API key (not saved)' -AsSecureString
$pointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($secureKey)
try {
    $env:SAYANGCARE__AI__API_KEY = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($pointer)
    $env:SAYANGCARE__AI__MODEL = Read-Host 'Groq model ID enabled for your account'
    if ([string]::IsNullOrWhiteSpace($env:SAYANGCARE__AI__MODEL)) {
        throw 'A model ID is required.'
    }
    $env:SAYANGCARE__AI__BASE_URL = 'https://api.groq.com/openai/v1/chat/completions'
    .\scripts\run-ai-evaluation.ps1 -Mode Real
} finally {
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($pointer)
    $secureKey.Dispose()
    $env:SAYANGCARE__AI__API_KEY = $oldKey
    $env:SAYANGCARE__AI__MODEL = $oldModel
    $env:SAYANGCARE__AI__BASE_URL = $oldEndpoint
}
```

By default this sends three sequential synthetic caller turns; add
`-RealAiCalls 10` to the wrapper for the latest sample size (allowed range 1..20).
Calls are paced at least five seconds apart. Each normally requires a
sentiment and reply provider operation. It may consume API credits. Only the
synthetic phrase "Hello, I would like to chat." and application prompts are
sent; no real caller data is used. This is a smoke sample, **not a statistical
p95/p99 qualification**. The existing application prompt/model protocol is part
of the measured path.

Evidence is saved under `artifacts\phase5-final\ai\<timestamp>-Real`:

- `real-ai.json`: mode, provider host/model, caller-turn percentiles, fallback
  count, stage means/outcomes, and `validation_passed`.
- `real-ai.prom`: per-stage duration histograms and outcomes.
- `real-ai.log`: local API diagnostics; review privately before sharing.

The turn budget is 5 seconds, sentiment cap 800 ms, reply cap 3 seconds, with
1 second reserved for persistence. A deadline/fallback can make the HTTP route
look fast despite provider failure. Therefore report provider successes,
timeouts, fallbacks, and stage measurements alongside HTTP latency.
Authentication errors, rate limits, unsupported models, malformed sentiment
JSON, all-fallback results, and missing credentials must remain failures.

Run offline gate tests without credentials:

```powershell
.\.venv\Scripts\python.exe -B scripts\test_perf_benchmark.py
.\.venv\Scripts\python.exe -B scripts\perf-benchmark.py --check-real-ai
```

The second command is expected to fail until the key is configured. Never
replace that failure with deterministic responses and call it a live AI result.
