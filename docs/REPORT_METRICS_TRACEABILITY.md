# Report metrics traceability

Verified against repository artifacts on 18 November 2026. All paths below
are relative to the repository root; links resolve from this document.
Commands are read-only and run in PowerShell from the repository root.

## Scope and evidence boundaries

- **Paper inspected:** [GROUP25_FINAL_REVISED.pdf](../documents/GROUP25_FINAL_REVISED.pdf),
  four pages. There is no current file named `GROUP25.pdf`. The reviewed PDF's
  SHA-256 is `03B50EC8C2E610F786DFCB8D40BE7F2F7A6BF21A86F0F97A9C8C6A1963D97720`.
  [GROUP25.md](GROUP25.md) is supporting source, not a byte-identical extraction.
- **Slides:** no `.pptx`, `.ppt`, `.odp` or `.key` deck was found in the
  workspace. Slide-ready figures and extra statistics below refer to sections
  7-9 of [PRESENTATION_AND_AI_HANDOFF.md](PRESENTATION_AND_AI_HANDOFF.md).
  An actual deck's slide numbers, screenshots and additional claims cannot
  be certified until that deck is available.
- The requested `docs/PHASE_5_LOCAL_RESULTS.md` and
  `docs/TEST04_AND_AI_VALIDATION.md` were removed during documentation cleanup.
  Their pre-cleanup versions were inspected in Git at `41c607c`. They are
  historical explanations, **not primary measurement artifacts**.
- The four-CPU suite and the later two-CPU PostgreSQL recovery suite used
  different images and resource conditions. Do not combine them into a
  claim that the latest revision passed every scenario in one experiment.
  Similarly, separate Raft process-fault observations from its later harness.
- Counts describe synthetic workflows, not people or real phone calls.
  `iterations.count` counts attempts; only call them completed lifecycles
  when the lifecycle/consistency checks also pass.
- JSON `http_failure_rate` and k6 `http_req_failed.value` are fractions:
  multiply by **100** for a percentage. HTTP p95/p99 are milliseconds, not
  whole-call duration. Whole caller-turn and inference-stage times are
  separately identified below. Rounded paper values do not replace raw values.
- A `k6_exit_code` of **99 remains a failed acceptance run**, even below 1%
  HTTP errors or with all acknowledged writes intact. The saved k6 Rate
  object's `passes`/`fails` labels can be counterintuitive for
  `http_req_failed`; use its `.value` for the HTTP failure fraction, not a
  guessed interpretation of those labels. Use recorded exit codes and explicit
  audit fields rather than treating a bare threshold Boolean as a verdict.

## 1. Master lookup table

`[]` denotes an array; selectors such as `[name=...]` mean filter by that
field, not a literal PowerShell expression. Multiple paths in one cell
distinguish raw client measurements from runner/audit metadata.

### A. Paper Table 1, abstract and Kubernetes slide results

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Table 1: baseline workload and completed lifecycles | 100 VUs / 30 s; **2,100**; exit **0** | [artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-report.json](../artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-report.json) | `vus`, `duration`, `iterations`, `k6_exit_code`; `lifecycle_success_rate = 1`, `consistency_passed = true` |
| Table 1: baseline HTTP p95/p99 and errors | **360/830 ms**, **0%**; raw 360.2514 / 830.405882 ms | [artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-k6.json](../artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-k6.json) | `metrics.http_req_duration["p(95)"]`, `["p(99)"]`; `100 * metrics.http_req_failed.value`; `metrics.iterations.count = 2100` |
| Table 1: API pod-loss workload and completed lifecycles | 100 VUs / 60 s; **4,410**; replacement Ready **8.903 s**; exit **0** | [artifacts/phase6-current/final-lifecycle/LIFE-pod-failover-20261103T084240Z-report.json](../artifacts/phase6-current/final-lifecycle/LIFE-pod-failover-20261103T084240Z-report.json) | `vus`, `duration`, `iterations`, `replacement_ready_seconds`, `k6_exit_code`; `fault` and `replacement_uid` identify the replacement |
| Table 1: API pod-loss HTTP p95/p99 and errors | **326/651 ms**, **0%**; raw 325.799855 / 651.105377 ms | [artifacts/phase6-current/final-lifecycle/LIFE-pod-failover-20261103T084240Z-k6.json](../artifacts/phase6-current/final-lifecycle/LIFE-pod-failover-20261103T084240Z-k6.json) | `metrics.http_req_duration["p(95)"]`, `["p(99)"]`; `100 * metrics.http_req_failed.value`; `metrics.iterations.count = 4410` |
| Baseline/API pod-loss independent SQL audit | Respectively **2,100/4,410** distinct archives and checkpoints; **0** invalid transcripts, nonterminal rows or pending checkpoints | [baseline report](../artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-report.json); [pod-loss report](../artifacts/phase6-current/final-lifecycle/LIFE-pod-failover-20261103T084240Z-report.json) (full paths immediately above) | `audit.rows`, `audit.distinct_ids`, `audit.checkpoints`, `audit.invalid_transcripts`, `audit.nonterminal`, `audit.pending`; `consistency_passed` |
| Table 1: Redis failover workload and completed lifecycles | 20 VUs / 120 s; **2,208**; recovery **8.157 s**; exit **0** | [artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-report.json](../artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-report.json) | `vus`, `duration`, `lifecycle_iterations`, `recovery.seconds`, `k6_exit_code`; `fault_mode = "master-pause"`, `fault.pause_ms = 60000`, `primary_change_observed = true` |
| Table 1: Redis HTTP p95/p99 and errors | **112/123 ms**, **0%**; raw 112.358195 / 122.867363 ms | [artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-k6.json](../artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-k6.json) | `metrics.http_req_duration["p(95)"]`, `["p(99)"]`; `100 * metrics.http_req_failed.value`; `metrics.iterations.count = 2208` |
| Redis acknowledged-operation consistency | **2,208/2,208/2,208** acknowledged creates/turns/hangups; no missing acknowledged durable operations | [artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-report.json](../artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-report.json) | `acknowledged_session_count`, `acknowledged_turn_count`, `acknowledged_hangup_count`; empty `missing_durable_ids`, `invalid_acknowledged_turn_ids`, `missing_acknowledged_archive_ids`; `durable_all_acknowledged = true`. `remaining_hot_keys = 87` is **not** zero and does not invalidate the acknowledged-operation audit |
| Table 1 / abstract: HPA workload and completed lifecycles | 100 VUs / 120 s; **9,248**; exit **0** | [artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json); [artifacts/phase6-current/final-hpa/20261103T085504Z-evaluation.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-evaluation.json) | k6: `metrics.iterations.count`, `metrics.call_lifecycle_success.value = 1`, `metrics.call_lifecycle_failures.count = 0`, `metrics.checks.fails = 0`; evaluation: `vus`, `load_duration`, `k6_exit_code` |
| Table 1 / abstract: HPA HTTP p95/p99 and errors | **282/507 ms**, **0%**; slide precision **281.6376/507.4391 ms** | [artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json) | `metrics.http_req_duration["p(95)"] = 281.63758999999993`, `["p(99)"] = 507.4391479999998`; `100 * metrics.http_req_failed.value = 0` |
| HPA scaling summary and delays | **3 -> 10 -> 3**; desired increase **50.5 s**; additional Ready **61.7 s**; cooldown **502.6 s** | [artifacts/phase6-current/final-hpa/20261103T085504Z-evaluation.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-evaluation.json); [CSV](../artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv) | `initial_replicas`, `max_desired_replicas`, `max_ready_replicas`, `first_scale_up_desired_seconds`, `first_additional_pod_ready_seconds`, `downscale_seconds_after_load`; final CSV row has all three replica columns = 3 |
| HPA exact acknowledged-ID SQL comparison | **9,248 each** SESSION/TURN/HANGUP; **9,248** archives/checkpoints; **0** invalid, nonterminal, pending or mismatched checkpoints | [artifacts/phase6-current/final-hpa/acknowledged-audit.json](../artifacts/phase6-current/final-hpa/acknowledged-audit.json); [artifacts/phase6-current/final-hpa/20261103T085504Z-sql-audit.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-sql-audit.json) | `stages[].stage`, `.acknowledged`, `.missing_ids` (empty), `.exact_set_match = true`; `archives`, `checkpoints`, `invalid_transcripts`, `nonterminal`, `pending`, `missing_or_mismatched_checkpoint`. SQL audit retains `ids[]`; client ACK records are in [20261103T085504Z-k6.stdout.log](../artifacts/phase6-current/final-hpa/20261103T085504Z-k6.stdout.log) |

### B. Figures and performance-refinement evidence

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Figure 1: architecture diagram | No empirical series; optional **3** Raft voters, majority **2** | [live-process-observations.json](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json); [restored-node-status.json](../artifacts/fencing/20261103T063451737Z-3f552c29/restored-node-status.json) | `environment.raft_ports[]` identifies three measured processes; restored array has three node snapshots. Majority is a protocol/design property, not a measured percentage |
| Figure 2: every replica-count point | **114** recorded samples; initial **3**, peak **10**, final **3** | [artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv](../artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv); plotted [phase6-hpa-replicas.png](../documents/phase6-hpa-replicas.png) | All CSV rows: X = `timestamp_utc` minus first row's timestamp, in seconds; Y = numeric `desired_replicas`, `current_replicas`, `ready_replicas`; `phase` distinguishes before-load/load/cooldown. Step lines between samples do not reveal exact event times |
| Figure 3, bar 1: Fencing / 2 CPUs | **1,511 ms** (raw **1510.552075**) | [artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073309Z-k6.json](../artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073309Z-k6.json); [matching report](../artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073309Z-report.json) | `metrics.http_req_duration["p(95)"]`; report `http_p95_ms`, `vus = 100`, `duration = "30s"`, `k6_exit_code = 0` |
| Figure 3, bar 2: Atomic finish / 2 CPUs | **2,294 ms** (raw **2293.64145**); latency gate **failed** | [artifacts/phase6-current/optimized/LIFE-baseline-20261103T075137Z-k6.json](../artifacts/phase6-current/optimized/LIFE-baseline-20261103T075137Z-k6.json); [matching report](../artifacts/phase6-current/optimized/LIFE-baseline-20261103T075137Z-report.json) | `metrics.http_req_duration["p(95)"]`; report `http_p95_ms`, `vus = 100`, `duration = "30s"`, `k6_exit_code = 99` |
| Figure 3, bar 3: Atomic finish / 4 CPUs | **211 ms** (raw **210.706035**) | [artifacts/phase6-current/four-cpu/LIFE-baseline-20261103T082512Z-k6.json](../artifacts/phase6-current/four-cpu/LIFE-baseline-20261103T082512Z-k6.json); [matching report](../artifacts/phase6-current/four-cpu/LIFE-baseline-20261103T082512Z-report.json) | `metrics.http_req_duration["p(95)"]`; report `http_p95_ms`, `vus = 100`, `duration = "30s"`, `k6_exit_code = 0` |
| Figure 3, bar 4: Final revision / 4 CPUs | **360 ms** (raw **360.2514**) | [artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-k6.json](../artifacts/phase6-current/final-lifecycle/LIFE-baseline-20261103T084205Z-k6.json) | `metrics.http_req_duration["p(95)"]`; same baseline as Table 1, **not** a fifth independent run |
| Figure 3 rendering input and dashed gate | **1511 -> 2294 -> 211 -> 360 ms**; dashed **2,000-ms** limit | [artifacts/phase6-current/baseline-figure-data.json](../artifacts/phase6-current/baseline-figure-data.json); plotted [phase6-evaluation.png](../documents/phase6-evaluation.png); [raw first-bar k6 summary](../artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073309Z-k6.json) | Figure-data array `[].label`, `[].source`, `[].p95_ms` are a derived index to the four raw runs above. Gate appears in k6 `metrics.http_req_duration.thresholds["p(95)<2000"]` **key**; it is an acceptance setting, not a measurement |
| Two-CPU sustained workload before refinement | HTTP p95 **2,220.53 ms**; pool timeouts; failed run | [artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073411Z-k6.json](../artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073411Z-k6.json); [report](../artifacts/phase6-current/before-capacity/LIFE-baseline-20261103T073411Z-report.json); [api-log.txt](../artifacts/phase6-current/before-capacity/api-log.txt) | `metrics.http_req_duration["p(95)"] = 2220.52562`; report `vus = 100`, `duration = "120s"`, `k6_exit_code = 99`; log text `pool timed out while waiting for an open connection` |
| Database wait and CPU-throttling diagnosis | Observed WAL synchronization waits and throttling; no causal percentage claimed | [artifacts/phase6-current/before-capacity/db-waits-samples.jsonl](../artifacts/phase6-current/before-capacity/db-waits-samples.jsonl); [node-quota-before.txt](../artifacts/phase6-current/node-quota-before.txt) | Each JSONL row: `utc`, `waits[].wait_event` (`WALSync`, `WALWrite`, `SyncRep`), `waits[].count`, `connections`; text counters `nr_throttled`, `throttled_usec`. These are snapshots/counters, not per-request wait durations |
| Recorded upstream HTTP error preceding keep-alive fix | **502** on an incoming request in the earlier HPA run | [artifacts/phase6-current/hpa/ingress-502-extract.txt](../artifacts/phase6-current/hpa/ingress-502-extract.txt); [acknowledged-audit.json](../artifacts/phase6-current/hpa/acknowledged-audit.json) | First log line: `2026-11-03T08:28:18... POST /api/v1/calls/incoming ... 502`; audit `classification`. This belongs to `HPA-20261103T082656Z`, **not** the final zero-error HPA run |

### C. PostgreSQL recovery table (paper section 5.3) and supporting slides

For these dependency reports, `recovery.seconds` measures the runner's observed
readiness/primary recovery after injection, not exact Raft election latency or
a claim that the database was unavailable for every millisecond of that period.

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Earlier four-CPU PostgreSQL replacement | **0.609%** HTTP failures; **18.662 s** recovery; exit **99**; **2,003 attempts**, **1,958** ACKs per stage | [artifacts/phase6-current/final-dependencies/DEP-postgres-20261103T084355Z-report.json](../artifacts/phase6-current/final-dependencies/DEP-postgres-20261103T084355Z-report.json); [raw k6](../artifacts/phase6-current/final-dependencies/DEP-postgres-20261103T084355Z-k6.json) | Report `100 * http_failure_rate = 0.6092143673`, `recovery.seconds`, `k6_exit_code`, `lifecycle_iterations`, `acknowledged_session_count`, `acknowledged_turn_count`, `acknowledged_hangup_count`; raw `metrics.http_req_failed.value`. Recovery/consistency pass does not make strict availability pass |
| Follow-up row 1 (`095201Z`, 20 VUs/120 s) | **0.157%**, **50.951 s**, exit **99**; ACKs **2,228/2,227/2,227** | [artifacts/postgres-recovery/DEP-postgres-20261103T095201Z-report.json](../artifacts/postgres-recovery/DEP-postgres-20261103T095201Z-report.json); [raw k6](../artifacts/postgres-recovery/DEP-postgres-20261103T095201Z-k6.json) | `vus`, `duration`, `100 * http_failure_rate = 0.1568803227`, `recovery.seconds`, `k6_exit_code`, `acknowledged_session_count`, `acknowledged_turn_count`, `acknowledged_hangup_count`; raw `metrics.http_req_failed.value` |
| Follow-up row 2 (`095656Z`, 20 VUs/120 s) | **0.228%**, **6.104 s**, exit **99**; ACKs **2,191/2,191/2,191** | [artifacts/postgres-recovery/DEP-postgres-20261103T095656Z-report.json](../artifacts/postgres-recovery/DEP-postgres-20261103T095656Z-report.json); [raw k6](../artifacts/postgres-recovery/DEP-postgres-20261103T095656Z-k6.json) | Same fields as preceding row; `100 * http_failure_rate = 0.2276867031` |
| Final follow-up row (`100531Z`, 20 VUs/120 s) / abstract | **2,216** completed lifecycles; **0%** HTTP failures; **7.160 s** recovery; exit **0**; ACKs **2,216/2,216/2,216** | [artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json](../artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json); [raw k6](../artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-k6.json) | `lifecycle_iterations`, `http_failure_rate`, `recovery.seconds`, `k6_exit_code`, all three `acknowledged_*_count` fields; `availability_thresholds_passed = true`, `recovery_criteria_passed = true`; raw `metrics.iterations.count`, `metrics.call_lifecycle_failures.count = 0`, `metrics.checks.fails = 0` |
| Final PostgreSQL HTTP latency | **23.11/85.90 ms** p95/p99 (raw **23.11488/85.899613**) | [artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-k6.json](../artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-k6.json); [report](../artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json) | `metrics.http_req_duration["p(95)"]`, `["p(99)"]`; report duplicates as `http_p95_ms`, `http_p99_ms` |
| Final PostgreSQL no missing acknowledged operations | **2,216** distinct terminal archives and valid durable turns; **0** invalid/nonterminal/pending or missing ACKs | [artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json](../artifacts/postgres-recovery/DEP-postgres-20261103T100531Z-report.json) | `audit.rows`, `audit.distinct_ids`, `audit.invalid_transcripts = 0`, `audit.nonterminal = 0`; `durable_audit.rows`, `durable_audit.valid_turn_ids` (length), `durable_audit.pending = 0`; empty `missing_durable_ids`, `invalid_acknowledged_turn_ids`, `missing_acknowledged_archive_ids`; `durable_all_acknowledged = true`, `archived_all_acknowledged = true` |
| Extra slide-guide baseline after PostgreSQL fix | 20 VUs/30 s; **580** lifecycles, **0%**, **29.2031/69.2641 ms**, exit **0** | [artifacts/postgres-recovery/LIFE-baseline-20261103T101041Z-report.json](../artifacts/postgres-recovery/LIFE-baseline-20261103T101041Z-report.json); [raw k6](../artifacts/postgres-recovery/LIFE-baseline-20261103T101041Z-k6.json) | `vus`, `duration`, `iterations`, `http_failure_rate`, `http_p95_ms`, `http_p99_ms`, `k6_exit_code`; this is **not** a rerun of the 100-VU Table 1 baseline |

### D. Test counts, original loss and inference

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Latest paper workspace and explicitly executed database tests | **115 passed, 8 ignored; 3 database integrations** | [artifacts/postgres-recovery/context.json](../artifacts/postgres-recovery/context.json) | `workspace_passed`, `workspace_ignored`, `session_store_database_integrations_passed`. This is a retained **consolidated record**, not full Cargo stdout; it does not enumerate every test |
| Older workspace checkpoint (not the latest paper total) | **113 passed, 7 ignored, 0 failed**; two named database integrations and keep-alive regression recorded as passed | [artifacts/phase6-current/validation-summary.txt](../artifacts/phase6-current/validation-summary.txt); [final-evaluation.json](../artifacts/phase6-current/final-evaluation.json) | First text line and subsequent named-test lines; JSON `validation.workspace_passed`, `.workspace_ignored`, `.workspace_failed`. Do **not** cite this file as proof of 115/8 |
| Original Redis-first acknowledged-creation loss | **17** missing out of **1,597** acknowledged creates | [artifacts/phase5-final/dependencies/DEP-redis-20261025T172859Z-report.json](../artifacts/phase5-final/dependencies/DEP-redis-20261025T172859Z-report.json); [missing-session-existence.csv](../artifacts/phase5-final/dependencies/DEP-redis-20261025T172859Z-missing-session-existence.csv) | `missing_acknowledged_ids` length = 17, `acknowledged_session_count = 1597`, `audit.rows = 1580`; CSV `ID`, `existsmaster`, `oldreplica`. The CSV records those two Redis checks, not three-node proof by itself. These are lost **creations**, not 17 acknowledged successful turns |
| Historical real Groq sample | **10** sentiment/reply pairs; median caller turn **1.13 s** (slide precision **1,126.83 ms**); max **1,443.03 ms**; **0** audio fallbacks | [artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json](../artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json) | `calls = 10`, `p50_turn_ms = 1126.8331999963266` (divide by 1000 for seconds), `max_turn_ms`, `audio_fallbacks`; `ai_stage_outcomes.sentiment.ok = 10`, `.reply.ok = 10`, `validation_passed = true` |
| Extra real-AI slide stage measurements | Mean sentiment/reply **570.95/514.43 ms**; one worker, historical model `openai/gpt-oss-20b` | [artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json](../artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json) | `stage_mean_ms.sentiment`, `stage_mean_ms.reply`, `workers`, `model`. Stage means are not whole-turn medians |
| Historical mock inference failure/recovery | **34** lifecycles; median/p95/max **40.22/158.50/371.66 ms**; **30** audio fallbacks | [artifacts/phase5-final/ai/20261025T184640Z-MockRecovery/breaker-recovery.json](../artifacts/phase5-final/ai/20261025T184640Z-MockRecovery/breaker-recovery.json) | `calls`, `p50_turn_ms`, `p95_turn_ms`, `max_turn_ms`, `audio_fallbacks`, `validation_passed = true`; `mode = "mock-dependency-recovery"`, `workers = 2`. Not real-provider tail latency |

### E. Raft ownership, faults and optional comparison-slide table

The early fault file explicitly identifies itself as **manually consolidated
observations from executed tool calls**. It is not a continuous raw fault
trace. Its automated HTTP companion and restored-node snapshots provide
separate retained evidence. Actual host-process fault ports were
18181-18183, not the normal Docker demo's 18081-18083.

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Concurrent claim race | Exactly **one 200** winner with generation **1**, competing **409** | [artifacts/fencing/20261103T063451737Z-3f552c29/report.json](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json); [requests.jsonl](../artifacts/fencing/20261103T063451737Z-3f552c29/requests.jsonl) | `assertions[name="raft FENCE-01 exactly one winner"].observed = [200,409]`; `requests[]` / JSONL filter `phase = "race"` and Raft-node `url`: `status`, `response.assignment_generation` |
| Ownership generations and stale owner rejection | Claim/transfer/release/reclaim **1/2/3/4**; resolve **5**; ABA stale/current notes **409/200** | [artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json); [HTTP report](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json) | `scenarios[name="browser controls with PostgreSQL-only ownership"]` generation fields; `scenarios[name="A-to-B-to-A fencing after recovery"].stale_note_status`, `.current_note_status`, `.stale_owner_generation = 1`, `.current_owner_generation = 3`; HTTP report retains automated transition requests for both modes |
| Leader loss with 2/3 voters alive | Leader **1 -> 2**, term **1 -> 2**; subsequent transfer **200**, generation **2** | [artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json) | `scenarios[name="leader failure with majority"]`: `leader_before`, `leader_after`, `term_before`, `term_after`, `post_election_transfer_status`, `post_election_generation`, `passed` |
| Quorum loss with 1/3 voters alive | New transfer/claim **503**; observed **8,123/8,056 ms** respectively | [artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json) | `scenarios[name="one voter cannot commit new ownership"]`: `transfer_status`, `fresh_claim_status`, `transfer_elapsed_ms`, `claim_elapsed_ms`. A 503 is an uncertain outcome; commands may commit after recovery |
| Rejoin convergence | All **3** nodes: leader **2**, term **2**, applied index **41**; pending **0** | [artifacts/fencing/20261103T063451737Z-3f552c29/restored-node-status.json](../artifacts/fencing/20261103T063451737Z-3f552c29/restored-node-status.json); [fault observations](../artifacts/fencing/20261103T063451737Z-3f552c29/live-process-observations.json) | Snapshot `[].status.node_id`, `.current_leader`, `.current_term`, `.last_applied`, `.last_log_index`; observation `scenarios[name="rejoin catches up and resolves ambiguous intents"].pending_after_recovery` |
| Independent ownership SQL audit | **8** Raft cases, **7** PostgreSQL cases; **0** pending and unfinished operations | [artifacts/fencing/20261103T063451737Z-3f552c29/fencing_live_raft-sql-audit.json](../artifacts/fencing/20261103T063451737Z-3f552c29/fencing_live_raft-sql-audit.json); [fencing_live_pg-sql-audit.json](../artifacts/fencing/20261103T063451737Z-3f552c29/fencing_live_pg-sql-audit.json) | Each file: `summary.cases`, `summary.pending`, `unfinished_operations`; `ownership[]` retains generations/coordinator, `notes[]` accepted notes |
| Later harness count (not a fault-injection run) | **411/411** assertions; **82** requests (**76** HTTP 200, **6** HTTP 409) | [artifacts/fencing/20261103T112552072Z-f5cd621e/report.json](../artifacts/fencing/20261103T112552072Z-f5cd621e/report.json) | `assertions` length, count where `.passed = true`; `requests` length and grouping by `.status`; `faults.raft` says not run; `comparison` says explicitly skipped; `not_covered[]` records coverage gaps |
| Slide claim-latency smoke | Raft median/p95 **81.30/82.57 ms**; PostgreSQL **59.94/61.30 ms** | [artifacts/fencing/20261103T063451737Z-3f552c29/report.json](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json); [raw requests.jsonl](../artifacts/fencing/20261103T063451737Z-3f552c29/requests.jsonl) | `latency.raft[operation="claim"]` and `latency.postgres[operation="claim"]`: `median_ms`, `p95_ms`, `attempted = successful = 3`; JSONL `milliseconds`, `operation`, `phase`, `url` |
| Slide transfer-latency smoke | Raft **72.49/81.48 ms**; PostgreSQL **50.54/51.24 ms** | [artifacts/fencing/20261103T063451737Z-3f552c29/report.json](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json) | Same fields, `operation = "transfer"` |
| Slide note-latency smoke | Raft **14.74/15.59 ms**; PostgreSQL **10.70/13.85 ms** | [artifacts/fencing/20261103T063451737Z-3f552c29/report.json](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json) | Same fields, **`operation = "notes"`** (plural) |
| Slide release-latency smoke | Raft **67.71/81.49 ms**; PostgreSQL **45.85/48.43 ms** | [artifacts/fencing/20261103T063451737Z-3f552c29/report.json](../artifacts/fencing/20261103T063451737Z-3f552c29/report.json) | Same fields, `operation = "release"` |

Only three sequential successful samples per operation were collected for
that comparison. Do not call it a statistically robust Raft-overhead
benchmark, production percentile, or proof Raft is necessary for fencing.

### F. Numerical context and non-empirical settings

These are included to prevent configuration numbers on slides being presented
as measured outcomes. Bibliographic years/page numbers, team identifiers and
protocol/port numbers are not experimental metrics.

| Metric/Experiment Name | Reported Value | Exact Source Artifact Path | JSON Key / CSV Column where the number is stored |
| --- | --- | --- | --- |
| Earlier CPU mismatch and subsequent allocation | Kubernetes advertised **16**, actual quota **2 -> 4 CPUs** | [artifacts/phase6-current/experiment-context.json](../artifacts/phase6-current/experiment-context.json); [node-quota-before.txt](../artifacts/phase6-current/node-quota-before.txt); [node-quota-after.txt](../artifacts/phase6-current/node-quota-after.txt) | Context `before.kubernetes_advertised_cpu`, `before.actual_node_cpu_quota`, `changes[]`; text `NanoCpus=2000000000` / `4000000000`, or cgroup `200000 100000` / `400000 100000` |
| Kubernetes topology counts | **3** API replicas at rest; configured **3** Redis, **3** Sentinel and **3** PostgreSQL instances | [artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv](../artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv); configuration [redis-sentinel.yaml](../k8s/base/redis-sentinel.yaml), [postgres.yaml](../k8s/base/postgres.yaml) | API: first CSV row `current_replicas`, `desired_replicas`, `ready_replicas` = 3. Other counts: StatefulSet `spec.replicas`, Cluster `spec.instances`. These manifests prove configuration, not that every dependency was Ready throughout every experiment |
| Later PostgreSQL test environment | **2 CPUs**, approximately **8 GiB** | [artifacts/postgres-recovery/context.json](../artifacts/postgres-recovery/context.json) | `docker_node_limits = "2000000000 8493465600"`: NanoCPUs and bytes (about 7.91 GiB); `scope`, `api_image`, `image_ids[]` distinguish this run from the earlier suite |
| Pool/HPA capacity model, not measured utilization | Pool **3**; HPA **3..20**; projected **(20+1+1)*3+16 = 82 < 100** DB connections | [artifacts/phase6-current/before-configuration.json](../artifacts/phase6-current/before-configuration.json); [experiment-context.json](../artifacts/phase6-current/experiment-context.json) | `pool`, `hpa.minReplicas`, `hpa.maxReplicas`; context `before.api_pool_per_pod`, `.postgres_max_connections`. **82 is calculated**, not a raw field; +1 rollout, +1 termination and 16 reserved are budgeting assumptions, not sampled concurrent use |
| Required synchronous replica acknowledgment | **1** required synchronous replica | [artifacts/phase6-current/optimized-configuration.json](../artifacts/phase6-current/optimized-configuration.json) | `database_synchronous.number = 1`, `.dataDurability = "required"`; `stop_delay_seconds = 90` is a shutdown allowance, not recovery time |
| Acceptance gates | Lifecycle/read-back > **99%**; failed lifecycle steps **0**; HTTP errors < **1%**; HTTP p95 < **2,000 ms** | [artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json](../artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json); [call-lifecycle.js](../tests/k6/call-lifecycle.js) | k6 threshold **keys** under `metrics.call_lifecycle_success`, `call_archive_consistency`, `call_lifecycle_failures`, `http_req_failed`, `http_req_duration`; source `options.thresholds`. Zero failed checks is additionally verified via `metrics.checks.fails` |
| Keep-alive refinement | Actix **75 s**, ingress upstream **60 s** | [artifacts/phase6-current/experiment-context.json](../artifacts/phase6-current/experiment-context.json); [validation-summary.txt](../artifacts/phase6-current/validation-summary.txt) | Context `changes[]` records settings; text names `upstream_connection_survives_the_previous_five_second_idle_limit` as passed. Settings/regression evidence, not a latency percentile |
| Other paper design constants | Projection bound **100 ms**; sweep **30 s**; inactive **5 min**; Redis TTL **1 h**; pool acquire **2 s**; create retry **5 s**; risk **1..5**, escalation **4..5**; **7** dashboard panels | No single raw measurement artifact asserts all these constants. Sources: [tiered.rs](../crates/session-store/src/tiered.rs), [sweeper.rs](../crates/session-store/src/sweeper.rs), [postgres.rs](../crates/session-store/src/postgres.rs), [sentiment.rs](../crates/api/src/sentiment.rs), [grafana-values.yaml](../k8s/monitoring/grafana-values.yaml) | Implementation/configuration facts, **not empirical results**. Source inspection is the proper proof; do not invent JSON keys or count a configured panel as proof of a historical live scrape |

**Evidence gaps to retain in presentation notes:** the current slide deck is
unavailable; the latest Cargo counts are retained in a consolidated context
record rather than complete test stdout; early process-fault observations
were manually consolidated; and later PostgreSQL fixes were not followed by
one complete, same-image rerun of the earlier HPA/API-loss suite in these
artifacts. None of these gaps should be silently converted into a pass.

## 2. Live Q&A Verification Commands

Run a single relevant line from the **repository root**. No containers,
Kubernetes connections, AI credentials or new workload are required. These
read small summaries or select fields from local JSON; they are intended for
quick on-screen proof, not a guaranteed two-second deadline on every laptop.
Missing files or keys are evidence gaps, not permission to substitute zero.

### Table 1: print raw counts, HTTP latency and failures

```powershell
Get-Content '.\artifacts\phase6-current\final-lifecycle\LIFE-baseline-20261103T084205Z-report.json' -Raw | ConvertFrom-Json | Select-Object run_id,vus,duration,iterations,http_p95_ms,http_p99_ms,http_failure_rate,k6_exit_code,consistency_passed
Get-Content '.\artifacts\phase6-current\final-lifecycle\LIFE-pod-failover-20261103T084240Z-report.json' -Raw | ConvertFrom-Json | Select-Object run_id,iterations,http_p95_ms,http_p99_ms,http_failure_rate,replacement_ready_seconds,k6_exit_code
Get-Content '.\artifacts\phase6-current\final-dependencies\DEP-redis-20261103T084638Z-report.json' -Raw | ConvertFrom-Json | Select-Object run_id,lifecycle_iterations,http_p95_ms,http_p99_ms,http_failure_rate,@{N='recovery_s';E={$_.recovery.seconds}},durable_all_acknowledged,k6_exit_code
(Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-k6-summary.json' -Raw | ConvertFrom-Json).metrics | Select-Object @{N='lifecycles';E={$_.iterations.count}},@{N='p95_ms';E={$_.http_req_duration.'p(95)'}},@{N='p99_ms';E={$_.http_req_duration.'p(99)'}},@{N='HTTP_error_percent';E={100*$_.http_req_failed.value}},@{N='failed_steps';E={$_.call_lifecycle_failures.count}}
```

### Figures 2 and 3, scaling delays and exact-ID consistency

```powershell
Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-evaluation.json' -Raw | ConvertFrom-Json | Select-Object initial_replicas,max_desired_replicas,max_ready_replicas,first_scale_up_desired_seconds,first_additional_pod_ready_seconds,downscale_seconds_after_load,k6_exit_code
Import-Csv '.\artifacts\phase6-current\final-hpa\20261103T085504Z-hpa-samples.csv' | Measure-Object
Import-Csv '.\artifacts\phase6-current\final-hpa\20261103T085504Z-hpa-samples.csv' | Select-Object -First 1 -Last 1 timestamp_utc,phase,current_replicas,desired_replicas,ready_replicas
Get-Content '.\artifacts\phase6-current\baseline-figure-data.json' -Raw | ConvertFrom-Json | ForEach-Object { $r=Get-Content -LiteralPath $_.source -Raw | ConvertFrom-Json; [pscustomobject]@{label=$_.label;source=$_.source;p95_ms=$r.http_p95_ms;exit=$r.k6_exit_code} } | Format-Table -AutoSize
Get-Content '.\artifacts\phase6-current\final-hpa\acknowledged-audit.json' -Raw | ConvertFrom-Json | Select-Object archives,checkpoints,invalid_transcripts,nonterminal,pending,missing_or_mismatched_checkpoint
(Get-Content '.\artifacts\phase6-current\final-hpa\acknowledged-audit.json' -Raw | ConvertFrom-Json).stages | Select-Object stage,acknowledged,exact_set_match,@{N='missing_count';E={@($_.missing_ids).Count}}
```

### PostgreSQL: show failed runs as well as the final pass

```powershell
Get-Content '.\artifacts\phase6-current\final-dependencies\DEP-postgres-20261103T084355Z-report.json' -Raw | ConvertFrom-Json | Select-Object run_id,@{N='HTTP_error_percent';E={100*$_.http_failure_rate}},@{N='recovery_s';E={$_.recovery.seconds}},k6_exit_code,durable_all_acknowledged
Get-ChildItem '.\artifacts\postgres-recovery\DEP-postgres-*-report.json' | ForEach-Object { Get-Content -LiteralPath $_.FullName -Raw | ConvertFrom-Json } | Select-Object run_id,lifecycle_iterations,@{N='HTTP_error_percent';E={100*$_.http_failure_rate}},@{N='recovery_s';E={$_.recovery.seconds}},k6_exit_code,acknowledged_session_count,acknowledged_turn_count,acknowledged_hangup_count | Format-Table -AutoSize
Get-Content '.\artifacts\postgres-recovery\DEP-postgres-20261103T100531Z-report.json' -Raw | ConvertFrom-Json | Select-Object http_p95_ms,http_p99_ms,availability_thresholds_passed,durable_all_acknowledged,@{N='missing_creates';E={@($_.missing_durable_ids).Count}},@{N='invalid_turns';E={@($_.invalid_acknowledged_turn_ids).Count}},@{N='missing_archives';E={@($_.missing_acknowledged_archive_ids).Count}},@{N='pending';E={$_.durable_audit.pending}}
```

### Workspace totals, historical loss, Raft and inference

```powershell
Get-Content '.\artifacts\phase6-current\validation-summary.txt'
Get-Content '.\artifacts\postgres-recovery\context.json' -Raw | ConvertFrom-Json | Select-Object workspace_passed,workspace_ignored,session_store_database_integrations_passed,docker_node_limits
Get-Content '.\artifacts\phase5-final\dependencies\DEP-redis-20261025T172859Z-report.json' -Raw | ConvertFrom-Json | Select-Object acknowledged_session_count,@{N='missing_ACK_creates';E={@($_.missing_acknowledged_ids).Count}},k6_exit_code
(Get-Content '.\artifacts\fencing\20261103T063451737Z-3f552c29\report.json' -Raw | ConvertFrom-Json).assertions | Where-Object name -eq 'raft FENCE-01 exactly one winner' | Select-Object name,passed,observed
(Get-Content '.\artifacts\fencing\20261103T063451737Z-3f552c29\live-process-observations.json' -Raw | ConvertFrom-Json).scenarios | Select-Object -First 4 | ConvertTo-Json -Depth 4
Get-Content '.\artifacts\fencing\20261103T063451737Z-3f552c29\restored-node-status.json' -Raw | ConvertFrom-Json | ForEach-Object { $_.status } | Select-Object node_id,state,current_leader,current_term,last_applied
Get-Content '.\artifacts\fencing\20261103T112552072Z-f5cd621e\report.json' -Raw | ConvertFrom-Json | Select-Object @{N='assertions';E={@($_.assertions).Count}},@{N='passed';E={@($_.assertions | Where-Object passed -eq $true).Count}},@{N='requests';E={@($_.requests).Count}},faults,comparison
(Get-Content '.\artifacts\fencing\20261103T063451737Z-3f552c29\report.json' -Raw | ConvertFrom-Json).latency | ConvertTo-Json -Depth 4
Get-Content '.\artifacts\phase5-final\ai\20261025T184504Z-Real\real-ai.json' -Raw | ConvertFrom-Json | Select-Object calls,p50_turn_ms,max_turn_ms,audio_fallbacks,ai_stage_outcomes,stage_mean_ms,validation_passed
Get-Content '.\artifacts\phase5-final\ai\20261025T184640Z-MockRecovery\breaker-recovery.json' -Raw | ConvertFrom-Json | Select-Object calls,p50_turn_ms,p95_turn_ms,max_turn_ms,audio_fallbacks,validation_passed
```

### Historical-guide and paper identity checks

```powershell
Get-FileHash '.\documents\GROUP25_FINAL_REVISED.pdf' -Algorithm SHA256
git --no-pager show '41c607c:docs/PHASE_5_LOCAL_RESULTS.md'
git --no-pager show '41c607c:docs/TEST04_AND_AI_VALIDATION.md'
```

The last two commands display older narrative documentation, not new
measurements, and do not restore the removed files. Prefer the selected raw
JSON fields for a short live answer.
