# SayangCare: presentation, demonstration and AI handoff

Prepared 16 November 2026 for Team 25, CSC3104 Cloud and Distributed Computing.
This is a reusable prompt document and rehearsal manual, not a new experiment.
Use it with the [IEEE article](GROUP25.md), [final PDF](../documents/GROUP25.pdf)
and [final Word report](../documents/GROUP25.docx).

## 1. How to use this document

1. Read sections 2-5 together so everyone explains the same architecture.
2. Rehearse section 6 using synthetic sessions only.
3. Use sections 7-10 to show where report numbers actually came from.
4. Assign the suggested speaking roles in section 11; these are not assertions
   about who implemented a component.
5. To generate slides with any AI, paste the master prompt in section 14,
   followed by this entire document and the article. Attach the referenced
   JSON/CSV files if the AI can read attachments.
6. Require the AI to distinguish measured, implemented, simulated and future
   functionality. A diagram or generated slide is not evidence of a test.

The report remains the single DOCX/PDF pair in `documents`. This Markdown guide
is intentionally separate from that pair and can be opened in VS Code preview.
It contains no credentials and should contain no real patient information.

## 2. Project identity and honest scope

**One-sentence pitch:** SayangCare is a distributed voice-support prototype
that preserves acknowledged conversation state, degrades safely when inference
fails, and prevents stale volunteers from modifying reassigned cases.

**Team:** Darren Tan Jun Le, Pieter Imanuel, Chew How Way, Jerlyn Ho, Eng Li Qi,
Lucas Ee and Goh Yi Xuan. Confirm spelling and speaking allocation before export.

**Problem:** A conversation spans multiple HTTP requests and can outlive an API
process. Dependencies fail or become slow. High-risk callers need human review.
Two volunteers must not overwrite each other's work after reassignment.

**Not claimed:** clinical validation, emergency-service readiness, a production
deployment, multi-zone resilience, arbitrary network-partition tolerance,
100 simultaneous real AI/telephone calls, or exactly-once carrier side effects.

### Submission constraints to preserve

The supplied Project 2026-27 guideline asks for an approximately 3,000-word
research article, at most six pages, in IEEE/ACM proceedings style, with an
abstract of at most 200 words. Include motivation, system design/figure,
implementation, quantitative evaluation/graphs, related work, conclusion/future
work and numbered cited references. The accompanying video is 10-15 minutes.

The guideline also requires `GROUP25_INSTRUCTIONS.pdf` separately. That is a
reproducibility/submission item, not a second version of the article. This guide
provides material for it but does not claim that required PDF was submitted.
Check the submission portal's required files and deadline with the module team.
No document or AI can guarantee a grade.

## 3. What we implemented

### 3.1 Components and responsibilities

```text
Dashboard JSON simulator              Twilio voice integration boundary
           |                         signed form callbacks / TwiML
           +--------------------+------------------+
                                |
                         Rust / Actix Web API
                                |
          +---------------------+---------------------+
          |                     |                     |
   PostgreSQL authority   Redis Sentinel        Inference provider
   checkpoints + CAS      hot projections       sentiment / reply
   transcripts/history    risk DESC / FIFO      separate breakers
   generation checks      recoverable cache     deadline + fallback
          |
          +-- PostgreSQL-only ownership (Kubernetes default)
          |
          +-- optional fixed 3-node OpenRaft ownership coordinator
                  majority 2/3, generations, idempotent commands

Kubernetes: API deployment/service, ingress, HPA, durable dependencies
Monitoring: custom Prometheus exporter, scraping, Adapter, Grafana
```

The seven Cargo crates are `api`, `core`, `session-store`, `circuit-breaker`,
`priority-queue`, `telephony` and `redis-client`. This is a modular monolith with
distributed dependencies, not seven independently deployed microservices.

### 3.2 Hot, durable active and cold state

- **Hot Redis projection:** low-latency, shared, best-effort session view and
  queue ordering. Redis replication is asynchronous, so an acknowledged Redis
  write alone is not a durable session guarantee.
- **Durable active checkpoint:** PostgreSQL stores the current versioned
  snapshot before a successful mutation response. Compare-and-swap prevents a
  stale request from replacing a newer transcript. Reads follow this authority.
- **Cold archive:** PostgreSQL JSONB transcripts preserve completed/abandoned
  conversations and assignment history for inspection.
- **Completion:** lock checkpoint, terminal transition, archive and checkpoint
  retirement occur in one transaction. Version-safe Redis cleanup follows.
- **Recovery:** the sweeper runs every 30 seconds, retries retained work and
  abandons calls inactive for five minutes. Redis projection TTL is one hour.
- **Boundary:** full periodic Redis reconciliation and automatic safe retention
  cleanup are not implemented. Terminal checkpoints/journals/tombstones can
  accumulate. Redis key count is not authoritative durable-session count.

Say: "We originally observed 17 acknowledged creations lost in a Redis-first
failover experiment. We changed the commit authority rather than claiming
Sentinel could make asynchronous writes durable."

### 3.3 Inference and degradation

The system has separate sentiment-scoring and reply-generation breakers,
shared bounded samples, latency/error tracking, Closed/Open/Half-Open states,
and one admitted recovery probe. There are response deadlines and bounded AI
concurrency. Three consecutive dependency errors can trip a breaker.

A local weighted phrase heuristic produces risk 1-5. Risk 4-5 takes immediate
human-escalation handling and bypasses generative inference. Cached audio or
deterministic text provides fallback. This is a demonstrable control path,
not a validated mental-health classifier. Browser speech or playback is not
proof that Twilio carrier audio or recognition works.

### 3.4 Ownership fencing: the reason for Raft

Redis still orders waiting cases by risk descending, then FIFO. Optional Raft
orders **ownership decisions**, not caller turns or Kubernetes restarts.

| Action on a fresh case | Owner | Generation |
| --- | --- | ---: |
| A claims | A | 1 |
| A transfers to B | B | 2 |
| B releases | none | 3 |
| A claims again | A | 4 |
| A resolves | none | 5 |

Protected notes, transfers, releases and resolves include the generation and a
stable request ID. PostgreSQL rejects an old token with HTTP 409:

```json
{"error":"Stale assignment generation (fencing token rejected)"}
```

The same volunteer returning later does not make an old token valid.
Release retains a generation tombstone instead of resetting to zero.

Raft mode's recovery protocol is:

```text
durable pending intent in PostgreSQL
 -> majority-committed expected-generation Raft transition
 -> atomic PostgreSQL ownership/history/checkpoint installation
 -> acknowledgement and best-effort Redis projection
```

A pending transition fences protected writes. Recovery uses the original
immutable request ID and payload. HTTP 503/timeout can mean an unknown outcome;
do not issue a replacement command with a new ID and assume the first aborted.
Raft-owned/pending cases never silently switch to PostgreSQL on quorum loss.
Existing PostgreSQL-owned cases retain their PostgreSQL coordinator.

Three fixed voters run on ports 18081-18083 in Docker, or isolated host
processes in the recorded process-fault experiment. Kubernetes HPA API pods
are **not voters**. They use the PostgreSQL-only ownership path by default.
Leader/term/applied indexes are diagnostics, not a linearizable follower-read
protocol. With two voters available, a leader can commit; with one, new
ownership decisions fail closed. An unrelated installed owner's note need not
be unavailable just because Raft lacks quorum.

PostgreSQL transactions can enforce the same generation rule. The honest
research comparison is explicit majority coordination, availability under
quorum loss, recovery and latency cost, not "PostgreSQL cannot lock".

### 3.5 Kubernetes, monitoring and capacity work

Implemented manifests and deployment include API Deployment/Service, NGINX
Ingress, Redis/Sentinel StatefulSets, persistent storage, CloudNativePG,
Prometheus, Adapter request-rate scaling and provisioned Grafana dashboards.
The recorded Minikube cluster had one physical Kubernetes node, three API
replicas at rest, three Redis, three Sentinels and three PostgreSQL instances.
Three pods on one host do not demonstrate three-host availability.

HPA allows 3-20 API replicas using CPU, memory and request-rate signals.
API pool size is three, acquisition timeout two seconds, database limit 100.
The recorded connection budget was
`(20 HPA + 1 surge + 1 terminating) * 3 + 16 reserved = 82`.
HPA adds pods, not host CPU. Actual Docker CPU quota must be checked separately
from the larger CPU count Kubernetes may advertise.

Targeted fixes included one terminal database transaction instead of three,
Actix keep-alive 75 seconds above ingress's 60-second upstream idle timeout,
writable-primary connection validation, replacement of dead/read-only pooled
connections, and clean fast CNPG shutdown. Only idempotent creation retries for
up to five seconds using the same stable SID. Other mutations are not blindly
replayed. Synchronous replication, WAL durability and pool bounds remain intact.

The actual exporter uses Rust's `prometheus` client, **not `actix-web-prom`**.
Dashboards show throughput, errors, p95/p99, Redis sessions, archive backlog,
breaker states and API replica availability. Alerts are durable/shared; the
prototype volunteer roster is still process-local.

## 4. Twilio explanation: use this wording

> We implemented and locally contract-tested signed Twilio webhooks, TwiML and
> outbound REST integration. Our trial setup used a US number. Local-access
> and trial constraints limited live Singapore testing, and our professor
> approved dashboard-based synthetic demonstrations. The dashboard exercises
> application workflows, not the real carrier path. Live inbound conversation
> and outbound ringing remain unvalidated. With funding, we would investigate
> a voice-capable Singapore number, satisfy availability and regulatory
> requirements, then test both directions end to end.

**Validated distinction:** the US number is the team's reported experience.
Do not say "Twilio free accounts universally only issue US numbers". Official
trial guidance documents verified-recipient and geographic restrictions, with
different current/legacy onboarding experiences. We did not independently
inspect a private Twilio account, billing record or carrier failure log.

Official sources, checked 16 November 2026:

- [Trial account guidance](https://www.twilio.com/docs/usage/tutorials/how-to-use-your-free-trial-account):
  trial calling/recipient restrictions; account capabilities can change.
- [Singapore voice guidelines](https://www.twilio.com/en-us/guidelines/sg/voice):
  domestic Singapore outbound calls should use a Twilio Singapore caller ID.
- [Singapore regulatory guidelines](https://www.twilio.com/en-us/guidelines/sg/regulatory):
  national numbers are not available to individuals; Level-6 numbers require a
  Singapore business address. Other number types have different requirements.

Funding alone does not guarantee inventory, voice capability, eligibility or
successful routing. A funded pilot would also require public HTTPS callbacks,
signature validation using the actual public URL, consent, budget, operator
procedures and inbound/outbound acceptance tests. Do not make paid calls during
a class demonstration without explicit team approval.

## 5. Implementation versus evidence versus remaining work

| Area | Implemented / demonstrated | Qualification |
| --- | --- | --- |
| Session durability | PostgreSQL checkpoint/CAS and transactional archive | Bounded synthetic fault evidence |
| Redis recovery | Sentinel promotion, version-safe cleanup | No full periodic reconciliation |
| API failover | Mid-call pod deletion through ingress, SQL audit | Pod loss, not machine/zone loss |
| Database recovery | Writable-primary pool validation, same-ID create retry | Later separate cluster/revision |
| HPA | 100-VU load and full cooldown | Earlier image, not all later fixes |
| Raft fencing | Races, stale-owner rejection, leader/quorum/rejoin evidence | Process faults and later harness are distinct |
| Monitoring | Custom exporter, Prometheus/Adapter/Grafana | Historical metrics may have expired |
| AI | Ten real pairs and mock recovery | Not 100 concurrent real AI calls |
| Telephony | Signed contracts/TwiML/outbound implementation | No live end-to-end voice claim |
| Privacy/authentication | Prototype controls only | Role tabs/volunteer IDs are not authentication |

Remaining engineering: final combined-image baseline/API-loss/HPA regression;
safe durable retention and anti-resurrection cleanup; comprehensive projection
reconciliation; multi-node/zone and backup restoration tests; authentication,
privacy/consent/security hardening; clinical evaluation; and funded telephony.
There is no transactional telephony outbox or exactly-once external call effect.

## 6. Practical demonstration: 3-4 minutes

### 6.1 Before recording

Coordinate access: this may be a shared development environment. Never stop
teammates' processes, remove PVCs/volumes or change cluster images casually.
Prepare a screen recording of the known-good demo as a fallback, visibly dated.

Keep three windows ready:

1. Dashboard with fresh synthetic call.
2. Terminal with read-only evidence commands from section 8.
3. Grafana plus the report's saved HPA figure/CSV.

From the repository root in Windows PowerShell, inspect before starting:

```powershell
git --no-pager log -1 --oneline
git status --short
kubectl --context minikube -n sayangcare get pods
kubectl --context minikube -n sayangcare get hpa
kubectl --context minikube -n sayangcare get deployment sayangcare-api -o wide
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api -o wide
```

Record running image IDs/source manifest and host resources if rerunning a
benchmark. A source checkout does not rebuild or replace running containers.
Use [the deployment guide](PHASE6_Windows.md) for controlled image rollout.
Do not claim a current Git HEAD is the source of every historical run.

For an already installed Kubernetes deployment:

```powershell
.\scripts\start-dashboard.ps1 -Mode Kubernetes -OpenBrowser
```

For the optional Docker Raft demonstration, only with permission to build/start
the Compose stack:

```powershell
.\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
```

Kubernetes normally uses local port 18080; Raft uses 18081-18083. Their
databases are separate. Do not demonstrate a Kubernetes-created session in
Docker and expect it to exist. Do not use 18081 for an ingress forward while
the Raft node occupies it. Keep launcher terminals open while using forwards.

### 6.2 Caller and persistence: approximately 45 seconds

1. Select **Caller lab**. Start a fresh synthetic session with **Start call**.
2. Record an ordinary turn with **Record turn** if deterministic/mock mode is
   configured. Otherwise avoid an unexpected external AI request.
3. Show **Call SID**, **Version**, **Turns** and the transcript.
4. Say: "The successful response follows a versioned PostgreSQL checkpoint.
   Redis is the fast projection, not the only copy."
5. End this ordinary case with **End call**. Show the terminal transcript.
   Use a separate fresh escalation for the ownership demo.

### 6.3 Escalation and fencing: approximately 90 seconds

1. In **Caller lab**, use **Simulate crisis call** to make a fresh synthetic
   escalation. It is a test phrase, not a real emergency.
2. In **Volunteer desk**, choose Aisha (`volunteer-aisha`, initially on shift).
   Claim the intended case with **Claim**, not a random **Claim next**.
3. Show owner and **Assignment generation = 1**. Capture the SID.
4. Transfer to Noor (`volunteer-noor`, initially on shift) with **Transfer**.
   Show generation 2 and the ownership authority. Ibrahim is initially off shift.
5. Use the retained-token HTTP request below to deliberately send generation 1.
   Show **409**, not just an error toast.
6. Send a generation-2 note for Noor and show **200** and unchanged ownership.
7. If time permits, **Release to queue** -> generation 3, claim -> 4,
   **Resolve** -> 5. Show history/audit.

Say: "This packet belongs to the old assignment. Even if it arrives late, the
storage layer rejects it. A Redis queue order or restarted pod cannot by itself
make a stale owner's packet safe."

Dashboard auto-refresh can replace an old token in another tab, so the HTTP
request is the reliable stale-token demonstration.

### 6.4 Copyable retained-token request

Set `$base` to the deployment you actually used and paste the fresh SID. The
following assumes Aisha claimed and transferred exactly once to Noor.
These requests create synthetic notes, so do not run against real cases.
Use fresh request IDs for distinct commands; preserve an ID for an uncertain retry.

```powershell
$base = 'http://localhost:18081'  # Raft; use 18080 for Kubernetes PG mode
$sid = 'PASTE-THE-FRESH-SYNTHETIC-CALL-SID'
$stale = @{
    assignment_generation = 1
    request_id = [guid]::NewGuid().ToString()
    body = 'Synthetic stale-owner demonstration'
} | ConvertTo-Json -Compress
$current = @{
    assignment_generation = 2
    request_id = [guid]::NewGuid().ToString()
    body = 'Synthetic current-owner demonstration'
} | ConvertTo-Json -Compress

# HttpClient exposes non-2xx bodies without treating expected 409 as a crash.
Add-Type -AssemblyName System.Net.Http
$client = New-Object System.Net.Http.HttpClient
$client.Timeout = [TimeSpan]::FromSeconds(15)
try {
    foreach ($attempt in @(
        @{ Volunteer = 'volunteer-aisha'; Json = $stale; Expected = 409 },
        @{ Volunteer = 'volunteer-noor'; Json = $current; Expected = 200 }
    )) {
        $url = "$base/api/v1/volunteers/$($attempt.Volunteer)/cases/$sid/notes"
        $content = New-Object System.Net.Http.StringContent(
            $attempt.Json, [Text.Encoding]::UTF8, 'application/json')
        try {
            $response = $client.PostAsync($url, $content).GetAwaiter().GetResult()
            try {
                $status = [int]$response.StatusCode
                $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                Write-Host "Expected $($attempt.Expected); actual $status"
                Write-Host $body
                if ($status -ne $attempt.Expected) { throw "Unexpected HTTP $status" }
            } finally { $response.Dispose() }
        } finally { $content.Dispose() }
    }
} finally { $client.Dispose() }
Invoke-RestMethod "$base/api/v1/calls/$sid/ownership"
Invoke-RestMethod "$base/api/v1/calls/$sid/ownership/audit"
```

The audit API exposes database-backed history, but is not independent
administrator SQL. The runners' SQL/acknowledged-ID audits provide separate
evidence. Do not show credentials or full sensitive transcript dumps.

### 6.5 Consensus and measured results: approximately 60 seconds

In the Raft dashboard show **Raft cluster**, leader, term and applied indexes.
For raw diagnostics:

```powershell
18081,18082,18083 | ForEach-Object {
    Write-Host "Node port $_"
    Invoke-RestMethod "http://localhost:$_/api/v1/queue/raft" |
        ConvertTo-Json -Depth 8
}
```

Say: "Three fixed voters require two to commit a new ownership change. The
recorded process test killed a leader, elected a new one, then removed quorum.
New ownership changes returned 503 until recovery. Rejoin converged at applied
index 41 in that run; 41 is not a permanent expected live value."

Show saved evidence rather than unexpectedly stopping the live cluster.
Do not claim the later 411-assertion run executed optional Docker faults.
Then show the HPA graph and run ID: "This earlier 100-VU, two-minute experiment
completed 9,248 lifecycles with zero HTTP failures. Replicas rose from 3 to 10
and returned to 3. It was not the exact later combined image."

## 7. Exact evidence map for the article

All paths below are relative to the repository root. Values are recorded
historical results, not promises about the machine currently running.

### 7.1 Main four-CPU dataset

Index: [final-evaluation.json](../artifacts/phase6-current/final-evaluation.json).
It records image `sayangcare-api:evaluated-20261103`, actual CPU quota 4 and
memory 4,089,446,400 bytes. Its earlier PostgreSQL result is still a failure;
the later passing experiment is in a different directory.

| Scenario / run | Lifecycles or attempts | HTTP p95 / p99 ms | HTTP failures | Recovery / exit |
| --- | ---: | ---: | ---: | --- |
| Baseline `LIFE-baseline-20261103T084205Z` | 2,100 | 360.2514 / 830.4059 | 0% | exit 0 |
| API pod loss `LIFE-pod-failover-20261103T084240Z` | 4,410 | 325.7999 / 651.1054 | 0% | Ready 8.903 s / 0 |
| Redis `DEP-redis-20261103T084638Z` | 2,208 | 112.3582 / 122.8674 | 0% | 8.157 s / 0 |
| HPA `HPA-20261103T085504Z` | 9,248 | 281.6376 / 507.4391 | 0% | 3 -> 10 -> 3 / 0 |
| Earlier PG `DEP-postgres-20261103T084355Z` | 2,003 attempts | 83.2690 / 180.8094 | 0.609214% | 18.662 s / 99 |

In the index use `scenarios[].attempts`, `http_p95_ms`, `http_p99_ms`,
`http_failure_rate`, `k6_exit_code`, `recovery_seconds` and `evidence`.
Multiply the stored failure-rate fraction by 100 for a percentage.

Individual lifecycle reports:

- `artifacts\phase6-current\final-lifecycle\LIFE-baseline-20261103T084205Z-report.json`
- `artifacts\phase6-current\final-lifecycle\LIFE-pod-failover-20261103T084240Z-report.json`
- `artifacts\phase6-current\final-dependencies\DEP-redis-20261103T084638Z-report.json`

HPA supporting files in `artifacts\phase6-current\final-hpa`:

- `20261103T085504Z-evaluation.json`: `initial_replicas`,
  `max_desired_replicas`, `max_ready_replicas`,
  `first_scale_up_desired_seconds = 50.5`,
  `first_additional_pod_ready_seconds = 61.7`,
  `downscale_seconds_after_load = 502.6`, `k6_exit_code = 0`.
- `20261103T085504Z-k6-summary.json`: HTTP metrics and iterations.
- `20261103T085504Z-hpa-samples.csv`: 114 timestamped samples, columns
  `timestamp_utc`, `phase`, `current_replicas`, `desired_replicas`,
  `ready_replicas`, `current_metrics_json`, `hpa_conditions`.
- `20261103T085504Z-sql-audit.json` and `acknowledged-audit.json`: 9,248
  archives/checkpoints, zero invalid/nonterminal/pending/mismatched work;
  exact create/turn/hangup acknowledged ID sets matched.

Do not use database row count alone as evidence of acknowledged-operation
preservation. A failed HTTP response can still have committed an operation.

### 7.2 Later PostgreSQL recovery: separate image/cluster

Directory: `artifacts\postgres-recovery`.
Context: Windows Minikube, actual two CPUs and 8,493,465,600 bytes RAM
(approximately 7.91 GiB), image `sayangcare-api:pg-recovery`, inference disabled.
Do not interpret the lower latency as a controlled four-CPU-versus-two-CPU
comparison; code, timing and configuration differ.

| Run suffix | HTTP failures | Recovery | Exit | ACK creates / turns / hangups |
| --- | ---: | ---: | ---: | --- |
| `095201Z` | 0.15688% | 50.951 s | 99 | 2,228 / 2,227 / 2,227 |
| `095656Z` | 0.227687% | 6.104 s | 99 | 2,191 / 2,191 / 2,191 |
| `100531Z` | 0% | 7.160 s | 0 | 2,216 / 2,216 / 2,216 |

Passing report:
`artifacts\postgres-recovery\DEP-postgres-20261103T100531Z-report.json`.
Its recorded metrics include `lifecycle_iterations = 2216`,
`http_failure_rate = 0`, `http_p95_ms = 23.11488`,
`http_p99_ms = 85.899613`, `k6_exit_code = 0`,
`recovery.seconds = 7.16`. The `audit` has `rows = distinct_ids = 2216`,
`invalid_transcripts = nonterminal = 0`, `min_version = max_version = 3`.
The report separately records acknowledged session/turn/hangup counts of
2,216 each and successful durability/archive checks.

`context.json` records workspace passes 115, ignored 8 and explicitly executed
session-store database integrations 3. These are recorded test categories;
do not add earlier 113 workspace tests to two integrations to invent 115.
Passing follow-up baseline:
`LIFE-baseline-20261103T101041Z-report.json`, 580 lifecycles, 0% errors,
p95 29.2031 ms, p99 69.2641 ms, exit 0.

### 7.3 Raft evidence: do not merge different test kinds

Directory:
`artifacts\fencing\20261103T063451737Z-3f552c29`.

- `report.json` and `requests.jsonl`: HTTP correctness/comparison observations.
- `live-process-observations.json`: explicit isolated-process fault evidence.
- `restored-node-status.json`: restored leader/term/applied snapshots.
- `fencing_live_raft-sql-audit.json` and
  `fencing_live_pg-sql-audit.json`: independent SQL evidence.
- `final-verification.json`: follow-up verification.

Observed: one concurrent claimant wins generation 1; transfer advances to 2;
stale generation gets 409, current gets 200; leader 1/term 1 changes to leader
2/term 2 after leader stop; quorum loss rejects new claim/transfer with 503;
rejoin converges to leader 2/term 2/applied 41. Independent SQL covered eight
Raft cases and seven PostgreSQL cases with no pending operations.

The fault observations used isolated host processes with temporary PostgreSQL
and Redis infrastructure, not a mid-presentation fault in the team's Docker
stack. In-process unit tests are a third, different category.

Three successful samples per operation were only a latency smoke:

| Operation | Raft median / p95 ms | PG median / p95 ms |
| --- | ---: | ---: |
| Claim | 81.30 / 82.57 | 59.94 / 61.30 |
| Transfer | 72.49 / 81.48 | 50.54 / 51.24 |
| Note | 14.74 / 15.59 | 10.70 / 13.85 |
| Release | 67.71 / 81.49 | 45.85 / 48.43 |

Do not present three samples or unlike deployments as statistically robust
Raft overhead. Report failures/timeouts separately rather than dropping them.

Later harness reports:
`artifacts\fencing\20261103T111523290Z-8b0cda48\report.json` and
`artifacts\fencing\20261103T112552072Z-f5cd621e\report.json`.
The latter records 411/411 assertions, 82 requests (76 HTTP 200, six HTTP 409).
Its outcome is "passed executed scenarios (see NOT RUN/N/A coverage)".
Optional fault injection was not run and PostgreSQL comparison was explicitly
skipped. A PostgreSQL-only deployment has no Raft election/quorum scenario.
Do not turn NOT RUN/N/A into a pass.

### 7.4 Historical AI evidence

- `artifacts\phase5-final\ai\20261025T184504Z-Real\real-ai.json`:
  ten real sentiment/reply pairs; median complete turn 1,126.83 ms, maximum
  1,443.03 ms; sentiment and reply success 10/10; no audio fallback;
  mean scoring 570.95 ms and reply 514.43 ms.
- `artifacts\phase5-final\ai\20261025T184640Z-MockRecovery\breaker-recovery.json`:
  34 mock recovery lifecycles; median 40.22 ms, p95 158.50 ms, max 371.66 ms.

These are October samples, not November 100-VU real-AI experiments. HTTP
request latency, whole caller-turn latency and inference-stage latency are
different measurements.

## 8. Show the professor the values: read-only commands

Run from the repository root. These commands read existing artifacts and do
not rerun a workload or call an external provider.

```powershell
$main = Get-Content '.\artifacts\phase6-current\final-evaluation.json' -Raw |
    ConvertFrom-Json
$main | Select-Object image,node_cpu_quota,node_memory_bytes
$main.scenarios | Select-Object scenario,run_id,attempts,http_p95_ms,http_p99_ms,
    @{Name='http_failure_percent';Expression={100 * $_.http_failure_rate}},
    recovery_seconds,k6_exit_code | Format-Table -AutoSize

$hpa = Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-evaluation.json' -Raw |
    ConvertFrom-Json
$hpa | Select-Object initial_replicas,max_desired_replicas,max_ready_replicas,
    first_scale_up_desired_seconds,first_additional_pod_ready_seconds,
    downscale_seconds_after_load,k6_exit_code | Format-List

$pg = Get-Content '.\artifacts\postgres-recovery\DEP-postgres-20261103T100531Z-report.json' -Raw |
    ConvertFrom-Json
$pg | Select-Object run_id,vus,duration,k6_exit_code
$pg.recovery | Format-List
$pg.audit | Select-Object rows,distinct_ids,invalid_transcripts,nonterminal,
    min_version,max_version
# Discover exact top-level summary fields without dumping thousands of IDs:
$pg.PSObject.Properties | Where-Object {
    $_.Name -match 'acknowledged|passed|http_|lifecycle_'
} | Select-Object Name,Value | Format-Table -AutoSize

$context = Get-Content '.\artifacts\postgres-recovery\context.json' -Raw |
    ConvertFrom-Json
$context | Select-Object workspace_passed,workspace_ignored,
    session_store_database_integrations_passed | Format-List

$audit = Get-Content '.\artifacts\phase6-current\final-hpa\acknowledged-audit.json' -Raw |
    ConvertFrom-Json
$audit | Select-Object run_id,archives,checkpoints,invalid_transcripts,
    nonterminal,pending,missing_or_mismatched_checkpoint
$audit.stages | Select-Object stage,acknowledged,missing_ids,exact_set_match

$samples = Import-Csv '.\artifacts\phase6-current\final-hpa\20261103T085504Z-hpa-samples.csv'
$samples | Select-Object -First 1
$samples | Select-Object -Last 1
$samples | Measure-Object
```

For Raft, avoid dumping `requests` or all response bodies during a talk:

```powershell
$raftPath = '.\artifacts\fencing\20261103T112552072Z-f5cd621e\report.json'
$r = Get-Content $raftPath -Raw | ConvertFrom-Json
$r | Select-Object run_id,outcome,exit_code
$r.configuration | Format-List
$r.not_covered | ConvertTo-Json -Depth 6
# Open these selected sections in VS Code to inspect their recorded schema:
# assertions, faults, restoration, remaining_stopped_containers
```

Open the earlier `live-process-observations.json` beside this later report and
explain why one is process-fault evidence and the other is a non-fault harness.
If a field is missing, inspect the saved JSON schema; do not silently substitute
zero, "passed" or another run's value.

## 9. Monitoring and recreating report graphs

### 9.1 Grafana and Prometheus

For an existing installation, use separate terminals and free ports:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:80
```

Open `http://127.0.0.1:13000/d/sayangcare-phase5` and
`http://127.0.0.1:19090`. Do not expose administrative secrets on screen.
Metrics endpoint: `/api/v1/metrics`.
Dashboard definition: [grafana-values.yaml](../k8s/monitoring/grafana-values.yaml).

| Panel | PromQL |
| --- | --- |
| Requests/s | `sum(rate(sayangcare_http_requests_by_route_total[5m]))` |
| 4xx/5xx % | `100 * sum(rate(sayangcare_http_requests_by_route_total{status=~"4..|5.."}[5m])) / clamp_min(sum(rate(sayangcare_http_requests_by_route_total[5m])), 1e-9)` |
| HTTP p95 seconds | `histogram_quantile(0.95, sum by (le) (rate(sayangcare_http_request_duration_by_route_seconds_bucket[5m])))` |
| HTTP p99 seconds | Same expression with `0.99` |
| Redis projection count | `max(sayangcare_redis_active_sessions)` |
| Archive backlog | `max(sayangcare_archive_backlog)` |
| Breakers | `max by (breaker) (sayangcare_circuit_breaker_state)` |
| Scraped API replicas | `count(up{namespace="sayangcare",pod=~"sayangcare-api-.*"})` |
| Healthy scrape targets | `sum(up{namespace="sayangcare",pod=~"sayangcare-api-.*"})` |

The error panel includes deliberate stale-token 409 responses; those are
correct rejections, not lost operations. Scrape-target counts are not exactly
Kubernetes Ready counts. Shared Redis counts use max, not a sum across replicas.
Prometheus histograms estimate quantiles and do not have to equal k6's
client-observed percentiles exactly.

For historical HPA, choose 3 November 2026 approximately 08:55-09:06 UTC only
if that data is still retained. If the time series expired, show saved CSV/JSON
and label the graph "recorded experiment", not live telemetry. No data is not
zero and must not be filled with fabricated points.

### 9.2 Recreate the HPA graph without inventing values

The report uses [phase6-hpa-replicas.png](../documents/phase6-hpa-replicas.png).
To reproduce with Excel/LibreOffice, import
`20261103T085504Z-hpa-samples.csv`:

1. Parse `timestamp_utc` as UTC, retaining the original timestamps.
2. X axis: elapsed seconds from the first sample.
3. Y series: numeric `desired_replicas`, `current_replicas`, `ready_replicas`.
4. Prefer a step plot; otherwise show sampled points and disclose interpolation.
5. Label axes "Elapsed time (s)" and "API replicas", not machines or users.
6. Include the run ID and 100 VUs/120 s in the caption, retaining full cooldown.
7. Check 114 samples, initial 3/3/3, peak 10 and return to 3.

The baseline comparison image is
[phase6-evaluation.png](../documents/phase6-evaluation.png).
Its caption explicitly says code and CPU resources changed together. For a
new chart, use section 8's actual JSON values; label p95/p99 in milliseconds,
plot the unchanged 2,000-ms gate, and keep failed runs visible.
Do not manufacture error bars or combine runs into an unmeasured mean.

## 10. Reproducing experiments after the presentation

These commands **mutate synthetic data, generate load and may disrupt shared
services**. They are instructions, not experiments executed for this document.
Obtain permission, verify context/images/resources, preserve old artifacts,
use dedicated output directories and inspect every native exit code.

Before tests, ensure deterministic/mock AI, appropriate available resources,
healthy primary/synchronous standby and no conflicting test. Read
[Phase 6 Windows](PHASE6_Windows.md) and each script's help.
Do not disable synchronous replication, weaken gates or increase pool sizes
without measured capacity evidence.

```powershell
$stamp = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$output = "artifacts\presentation-rerun-$stamp"

# Baseline; runner owns its ingress forward.
.\tests\k6\evaluate-lifecycle.ps1 -Mode baseline -Vus 100 -Duration 30s `
    -OutputDirectory "$output\baseline"
$baselineExit = $LASTEXITCODE
Write-Host "Native/runner exit: $baselineExit"

# Deletes an API pod while sessions run. Review confirmation prompt.
.\tests\k6\evaluate-lifecycle.ps1 -Mode pod-failover -Vus 100 -Duration 60s `
    -OutputDirectory "$output\pod-failover"

# PostgreSQL primary replacement, not deletion of its PVC.
.\tests\reliability\evaluate-dependencies.ps1 -Dependency postgres `
    -Vus 20 -Duration 120s -OutputDirectory "$output\postgres"

# Bounded Redis master pause to exercise Sentinel promotion.
.\tests\reliability\evaluate-dependencies.ps1 -Dependency redis `
    -FaultMode master-pause -PauseMilliseconds 60000 -Vus 20 -Duration 120s `
    -OutputDirectory "$output\redis"
```

Scripts may throw on a failed native process; retain the generated report's
`k6_exit_code` and explicit audit outcome, not only an outer shell's last value.
Never use `-AllowTransientFailures` to call a strict availability failure a pass.

HPA needs a caller-owned ingress forward. In a separate terminal:

```powershell
kubectl --context minikube -n ingress-nginx port-forward --address 127.0.0.1 svc/ingress-nginx-controller 18193:80
```

Then:

```powershell
.\tests\k6\evaluate-hpa.ps1 -BaseUrl 'http://127.0.0.1:18193' `
    -Vus 100 -Duration 120s -DownscaleTimeoutSeconds 900 `
    -OutputDirectory "$output\hpa"
```

Leave it running through full cooldown. The ordinary dashboard launcher uses a
Service forward that binds a backend pod: do not use that forward as proof of
stable API-pod failover. The fault runners use ingress to survive API deletion.
Stop only forwards/processes you own when finished; never kill by process name.

Raft comparison, with both deployments prepared:

```powershell
# Creates fresh synthetic cases but does not inject container faults.
.\scripts\test-raft-fencing.ps1 -SampleCount 10 -WarmupCount 2

# Optional disruptive run: verify actual Compose project first.
# Replace the placeholder; do not blindly copy a guessed project name.
.\scripts\test-raft-fencing.ps1 -InjectFaults -ComposeProject 'YOUR_VERIFIED_PROJECT'
```

Fault injection requires approval. Check `restoration`,
`remaining_stopped_containers`, unresolved operations and NOT RUN/N/A fields.
Do not remove volumes. Allow background recovery and verify all three voters
catch up after restarting only members this experiment stopped.

Automated regression on dedicated infrastructure:

```powershell
cargo test --workspace --locked
cargo test -p sayangcare-session-store --locked transactional_fencing_and_ambiguous_commit_recovery -- --ignored
cargo test -p sayangcare-api --locked http_claim_race_transfer_notes_and_stale_owner_are_fenced -- --ignored
```

The ignored database tests require private `TEST_DATABASE_URL` and
`TEST_REDIS_URL` for disposable infrastructure. Never paste credentials into
slides. An ignored test is not a pass.

Optional AI runs, separate from deterministic load:

```powershell
.\scripts\run-ai-evaluation.ps1 -Mode MockRecovery
# Explicitly approved external provider use only; consumes quota:
.\scripts\run-ai-evaluation.ps1 -Mode Real -RealAiCalls 3
```

Acceptance remains lifecycle success >99%, zero failed lifecycle/check steps,
read-back consistency >99%, HTTP failure rate <1% and HTTP p95 <2,000 ms.
An HTTP failure rate below 1% can still fail the zero-failed-step requirement.
k6 exit 99 is a failure, even when every acknowledged operation is preserved.
Record completed calls, attempts, failed/retried requests, recovery definition,
exact acknowledged IDs, valid transcript contents, duplicates, versions,
pending work and exit codes. Keep unsuccessful runs beside successful ones.

## 11. Suggested seven-member 13-minute presentation

This allocates speaking duties only. Confirm actual contributions separately.
Target 12-14 minutes to leave margin within the 10-15-minute requirement.

| Time | Speaker (suggested) | Slide / action | Key statement |
| --- | --- | --- | --- |
| 0:00-1:15 | Darren | 1-2: problem, objectives, boundaries | Reliable state and ownership, not clinical AI |
| 1:15-2:45 | Pieter | 3: architecture | Separate durable state, projections and coordination |
| 2:45-4:15 | Lucas | 4: sessions/database recovery | Commit before ACK; same-ID creation retry only |
| 4:15-5:30 | Eng Li Qi | 5: breaker/AI | Bound slow dependencies; heuristic is not diagnosis |
| 5:30-7:00 | Jerlyn | 6: ownership/fencing | Old generation rejected at protected storage |
| 7:00-8:30 | Chew How Way | 7-8: Kubernetes/metrics/results | HPA is pods; saved dataset is revision-specific |
| 8:30-10:45 | Goh Yi Xuan | 9: live dashboard + stale request | Synthetic workflow, observable 409/200 |
| 10:45-11:45 | Darren | 10: Raft fault evidence | Process faults separate from assertion harness |
| 11:45-13:00 | Goh Yi Xuan, brief handoff to team | 11-12: limitations/conclusion | Funded SG telephony and final-image regression |

### Suggested narration

**Darren, opening:** "SayangCare asks how a multi-request support conversation
can survive a failed process and how a delayed volunteer request can be stopped
after reassignment. We built a prototype and evaluated synthetic workflows.
We do not claim a validated crisis service."

**Pieter:** "PostgreSQL is the commit authority. Redis accelerates projections
and orders waiting cases. Optional fixed-node Raft coordinates ownership, not
all data. Kubernetes replaces processes; it cannot reconstruct uncommitted
application state or make an old ownership token current."

**Lucas:** "We learned from failure: a Redis-first run lost 17 acknowledged
creations. Durable versioned checkpoints corrected that authority boundary.
Later writable-primary checks and bounded same-ID creation retry gave a
separate primary-replacement pass: 2,216 lifecycles, zero HTTP failures and
7.16 seconds observed recovery. We kept the failed runs and synchronous
durability. This does not promise zero errors during arbitrary outages."

**Li Qi:** "Scoring and replies have separate breakers and bounded concurrency.
A failed provider must not indefinitely block a response. Local high-risk
heuristics bypass generation for human review, but cannot establish clinical
safety. Our ten real inference pairs are only a small smoke, separate from
100-VU deterministic benchmarks."

**Jerlyn:** "A claims generation 1; transfer to B produces generation 2. A's
delayed generation-1 request gets 409 at the protected resource. PostgreSQL
can implement fencing alone. Raft adds explicit majority-backed ownership
decisions and a quorum-availability trade-off, not magic Redis recovery."

**How Way:** "Our earlier four-CPU HPA experiment completed 9,248 lifecycles
at 100 virtual users, p95 282 milliseconds, zero HTTP failures, and 3-to-10-to-3
replicas. This figure includes cooldown. The exact later combined revision
still needs that full regression; we do not relabel an old image as current."

**Yi Xuan, demo:** "This is a synthetic call and its transcript/version.
Now Aisha claims, Noor receives the transfer, and the token advances. Here is
an intentionally delayed old-token note: 409. The current owner succeeds:
200. This is application correctness evidence, not proof of a carrier call."

**Darren, consensus evidence:** "The saved process experiment records a leader
change with two voters remaining, rejection of new ownership changes with only
one, and rejoin convergence. The later 411 assertions did not run optional
Docker faults; we keep those evidence categories separate."

**Closing:** "We attempted Twilio with a US trial number. The professor-approved
dashboard lets us demonstrate the distributed-system workflows within scope.
A funded Singapore pilot still needs a suitable available number, regulatory
eligibility, HTTPS callbacks and live validation. Our contribution is measured
integration with explicit failure boundaries, not a production-readiness claim."

## 12. Professor Q&A

**Why Raft instead of Redis?** Redis orders the best-effort queue. Sentinel
asynchronous replication is not a majority-committed ownership protocol.
Fencing belongs at the protected resource, not just in a cache lock.

**Why Raft instead of PostgreSQL?** PostgreSQL can enforce generations and is
a valid simpler alternative. We retain it as the baseline. Raft makes
majority ownership explicit and permits experiments on election/quorum
behaviour, at coordination and operational cost. We do not claim necessity.

**Is this Raft really durable if PostgreSQL is needed?** Raft and SQL have
separate responsibilities but both are dependencies of this implementation.
The SQL intent journal and atomic installation address the commit gap.
This is not a globally atomic transaction or arbitrary-partition proof.

**Does Kubernetes solve the same issue?** It restores process availability,
not assignment ordering, application state durability or stale-request safety.

**Where is a node?** A fixed Docker API/Raft voter or isolated test process;
three voters on one laptop are not independent physical fault domains.

**Why not elect more HPA pods as voters?** Dynamic API scaling would entangle
application capacity with consensus membership. Fixed voters keep quorum
membership separate; Kubernetes mode currently uses PostgreSQL-only ownership.

**What if a request times out after Raft commits?** Treat it as unknown, inspect
pending/ownership state and retry the same ID/payload. Recovery installs the
committed outcome. Do not infer rollback from timeout.

**Did you do 100 phone calls?** No, 100 concurrent virtual users exercised
synthetic JSON lifecycles. Real AI and real carrier tests are different.

**How can recovery take seven seconds with no failed requests?** Measured
primary recovery is not identical to user-visible failed responses. Other
requests, timing and bounded idempotent creation retries can hide a bounded
interruption in one experiment; longer outages can still fail.

**Are zero HTTP errors and no data loss the same?** No. A response can fail
after commit, or requests can fail without losing acknowledged state. We
evaluate availability and exact acknowledged-operation consistency separately.

**Is p95 a maximum?** No. Five percent of observations are above p95; p99 is
another percentile. Neither is a maximum or a confidence interval.

**What remains?** Final-image sustained regression, retention/reconciliation,
real telephony, physical fault domains, backup restore, privacy/authentication
and clinical evaluation. See section 5 rather than claiming "everything done".

## 13. Final rehearsal and evidence checklist

- Article <=6 pages, abstract <=200 words, readable figures and cited sources.
- One report DOCX and PDF, same title/authors/results and Twilio qualifications.
- Every number has a run ID, units, workload, configuration and source.
- Show 9,248 as HPA lifecycles, not requests, callers served or machine count.
- Show later PostgreSQL results as a different image/cluster.
- Show failed exit-99 experiments, not just the successful final attempt.
- No NOT RUN assertion described as an executed fault.
- No claims of final-revision HPA revalidation without a new saved run.
- Fresh synthetic case; correct deployment; both volunteers on shift.
- Old token retained manually; 409 and current-owner 200 observable.
- Backup screen recording and raw saved artifacts ready.
- No credentials, real phone numbers, private console or real transcripts shown.
- Seven members speak; 10-15-minute final video; actual contributions confirmed.
- Required reproducibility instructions and submission files checked separately.

## 14. Copy-and-paste master prompt for a slide-generation AI

Copy the following block, then append this entire guide and the final article.
If the AI cannot inspect files, the guide is its evidence summary; it must
not claim it independently verified the repository.

```text
You are helping Team 25 present SayangCare for CSC3104 Cloud and Distributed
Computing, AY2026/27. Create a submission-quality 10-15-minute presentation,
target 13 minutes, for seven speakers. Use the attached IEEE report and the
complete project/evidence/demo guide below as the authoritative context.

DELIVERABLES
1. If your environment supports it, generate an editable PowerPoint .pptx
   containing 12 main slides plus clearly separated backup slides. Otherwise
   produce a complete slide-by-slide specification, not a pretend download.
2. Every main slide needs a title, 3-5 concise bullets, a diagram/chart or
   screenshot plan where useful, exact speaker notes, speaker name, duration,
   transition, and a short evidence footer with run ID/path.
3. Include a separate timestamped demonstration script, exact UI actions,
   the stale-token 409/current-token 200 sequence, and a fallback video plan.
4. Include appendix slides for reproducible PowerShell commands, evidence
   traceability, exact metrics/units, Raft fault categories, acceptance gates,
   failed runs, limitations and anticipated professor questions.
5. Provide an evidence checklist and list unresolved submission items.

TEAM AND TIMING
Darren Tan Jun Le, Pieter Imanuel, Chew How Way, Jerlyn Ho, Eng Li Qi,
Lucas Ee, Goh Yi Xuan. Use the suggested speaking allocation in the guide,
but label this a proposed presentation allocation, not verified authorship
of individual code. All seven must speak meaningfully. Target 13 minutes,
with a 2-3-minute live demo; do not schedule a full HPA cooldown live.

MAIN SLIDE ORDER
1 Title and contribution.
2 Motivation, reliability objectives and non-clinical scope.
3 Architecture with request/state flow and four-way responsibility boundary.
4 Durable session lifecycle, failure lesson and PostgreSQL recovery.
5 Circuit breaker, bounded inference and small historical AI evaluation.
6 Majority ownership, token generations and stale-owner fencing.
7 Kubernetes/HPA/observability and database capacity budget.
8 Quantitative baseline/failover/HPA results with provenance.
9 Live dashboard demo: synthetic escalation, claim, transfer, 409 and 200.
10 Separate Raft process-fault observations and later correctness harness.
11 Twilio constraints, unvalidated boundaries and remaining engineering.
12 Conclusion with measured contribution and future work.

FACTUAL RULES
- Modular monolith, seven crates; not seven independently deployed services.
- PostgreSQL checkpoints are commit authority. Redis is a hot projection and
  risk-descending/FIFO queue. Cold archives are PostgreSQL JSONB.
- Three fixed Raft voters coordinate ownership, not queue ordering or all
  sessions. Quorum is 2/3. HPA APIs are not Raft voters.
- Generation sequence for a fresh case: claim 1, transfer 2, release 3,
  reclaim 4, resolve 5. PostgreSQL rejects stale mutations with HTTP 409.
- PostgreSQL alone can enforce fencing. Do not say Raft is uniquely necessary,
  faster, or a replacement for PostgreSQL, Redis Sentinel or Kubernetes.
- Explain the durable intent -> Raft commit -> SQL installation gap and
  unknown outcomes; do not promise a cross-system transaction.
- Main HPA evidence: earlier four-CPU image, 100 VUs/120 seconds, 9,248
  lifecycles, zero HTTP failures, HTTP p95/p99 281.6376/507.4391 ms,
  3->10->3 replicas, desired/extra Ready 50.5/61.7 seconds, cooldown 502.6.
- Later PostgreSQL experiment is a different two-CPU image/cluster:
  2,216 lifecycles, zero HTTP failures, HTTP p95/p99 23.1149/85.8996 ms,
  primary recovery 7.16 seconds, exit 0 and exact ACK preservation.
- Keep earlier failed PostgreSQL experiments and exit 99 visible.
- Latest recorded workspace count is 115 passed, eight ignored; three
  explicit session-store integrations are a separate category.
- Raft live process-fault observations and the later 411-assertion harness
  are different runs. Optional Docker faults/PostgreSQL comparison were
  skipped in the later harness. Never call skipped coverage a pass.
- The tiny three-sample Raft latency smoke is not a statistical benchmark.
- Ten real AI pairs and 34 mock recovery lifecycles are historical small
  samples, not 100 simultaneous AI calls.
- The exporter is custom Rust prometheus-client instrumentation, not
  actix-web-prom. Redis projection counts are not durable session counts.
- 100 VUs are virtual users, not real telephone calls. HTTP latency,
  caller-turn latency, inference timings and recovery are distinct.
- Team reports attempting Twilio with a US trial number. Do not universalize
  that every free account only supplies US numbers. Cite official trial
  restrictions and Singapore voice/regulatory guidance.
- Dashboard is approved synthetic demonstration scope, not live carrier
  validation. Funded Singapore voice is conditional on inventory, capability,
  regulatory eligibility and end-to-end testing, not payment alone.
- One-node Minikube does not prove host/zone HA. No clinical safety,
  production readiness or exactly-once telephony claim.
- Retention, full projection reconciliation and exact final-image HPA/API-loss
  regression remain limitations. Do not claim those are already completed.

VISUAL AND EVIDENCE RULES
Use professional readable IEEE-aligned slides, restrained colors and large
labels. Draw an editable architecture diagram and generation timeline.
Use the saved timestamped CSV for the HPA replica chart and real JSON values
for results. Show units, workload, date/run ID and configuration in captions.
Distinguish current live observations from historical saved evidence.
Do not invent screenshots, data points, quotations, experiment repetitions,
confidence intervals, teammate contributions, credentials or clinical claims.
If an attachment cannot be opened, say so and use the explicitly provided
values with that qualification. Mark any missing evidence for team review.
Preserve official reference links and cite report references where relevant.
Do not guarantee a grade or imply the submission has already been accepted.

Now produce the presentation and supporting notes using the complete guide
and article that follow.
```
