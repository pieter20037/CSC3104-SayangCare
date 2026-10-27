# Professor consultation: individual contributions and monitoring walkthrough

Prepared 27 October 2026 (Singapore time).

Use this alongside [the main consultation playbook](consultation.md).
This brief distinguishes assigned responsibilities from reported completed work
and team integration results. An assignment alone is not proof of completion.
The numerical results below are recorded experiments, not tests freshly run for
this document.

## 1. Opening explanation

> Our team tested SayangCare at three levels: individual components, the
> caller-to-volunteer workflow, and distributed behavior under load and failure.
> We checked both request success and preservation of acknowledged conversation
> data. Different members owned different areas, with follow-up integration
> fixes and reruns across those areas. We retain failed results and explain
> the remaining limitations.

## 2. Individual responsibilities and contributions

### Jerlyn Ho: queue correctness, concurrency and scaling evaluation

**Assigned:** FIX-02, TEST-01, TEST-02, TEST-05.

- Queue claim: handle stale/already-claimed entries without an unnecessary 409.
- k6 baseline: exercise concurrent session creation, turns and hangup.
- Pod failover: remove an API pod while traffic continues.
- HPA evaluation: measure replica growth/reduction alongside errors and latency.

**What to say:**

> Jerlyn's assigned area was queue and load testing. It covered queue-claim
> correctness, concurrent caller benchmarks, API pod-failure testing and HPA
> evaluation. These experiments assess service behavior under concurrency and
> failure, rather than only testing a single successful request.

**Team evidence:** baseline 1,358 lifecycles with zero HTTP failures; corrected
pod-loss run 2,090 lifecycles with zero HTTP failures; HPA scaled 3 -> 5 -> 3,
but strict load gates failed.

**Attribution:** Chew How Way reported later queue repairs, shutdown repair,
test-harness improvements and corrected reruns. Do not attribute every final
implementation or measurement exclusively to the assigned owner.

### Lucas Ee: archival, shared alerts and dependency reliability

**Assigned:** FIX-01, FIX-03, TEST-04.
**Previously documented:** DATA-05 archival retry/data-loss prevention.

- Recover escalated/terminal sessions when archival fails.
- Persist operator alerts in shared storage so pods see the same records.
- Test Redis and PostgreSQL recovery during synthetic traffic.
- Earlier archival work: transient-error retry, archive-before-cleanup and
  version-safe deletion.

**What to say:**

> Lucas's assigned area was data reliability: recoverable archival, cross-pod
> alerts and dependency-failure testing. His earlier DATA-05 implementation
> supplied retry and version-safe cleanup around archival.

**Tests:** failed archival and recovery, protection of newer versions, shared
alerts across API replacement, and acknowledged-operation audits after faults.

**Attribution:** the later PostgreSQL checkpoint redesign and database
shutdown tuning are additional work reported by Chew How Way, not automatically
part of Lucas's original archival implementation.

### Goh Yi Xuan: API metrics, Prometheus and circuit breakers

**Assigned:** API-04, OBS-01.
**Previously documented:** CB-01 and CB-02.

- Expose API measurements and extend Prometheus scraping.
- Observe request rates, status codes, latency and circuit-breaker state.
- Earlier breaker implementation: failure/latency thresholds, distress-aware
  behavior, state transitions and controlled recovery permits.

**What to say:**

> Yi Xuan's Phase 5 responsibility was API metrics and Prometheus scraping.
> His earlier circuit-breaker work also provided the dependency-protection
> foundation. Monitoring lets us observe the behavior; the breaker logic
> controls whether inference requests are allowed.

**Tests:** metrics availability/scraping, breaker transitions, threshold
behavior, stale-request isolation and recovery-probe control.

**Attribution:** Chew How Way reported subsequent metric extensions and a fix
for stale breaker metrics.

### Eng Li Qi: caller-turn latency and AI guardrails

**Assigned:** PERF-01 and PERF-02.

- Break caller-turn latency into stages.
- Bound inference waits and fallback behavior.
- Evaluate concurrency/resource limits and identify bottlenecks.

**What to say:**

> Li Qi's assigned area was latency and AI guardrails. The aim was to explain
> where a caller turn spends time and ensure a slow provider does not cause
> unbounded inference waiting. This also separates provider latency from
> database and coordination delays.

**Tests:** controlled slow/unavailable inference, bounded fallback, concurrency
behavior and stage measurements.

**Boundary:** the sustained-load pool timeouts remain unresolved. Do not claim
that adding API replicas automatically solved database capacity.

### Chew How Way: Grafana and cross-cutting integration/validation

**Assigned:** OBS-02, with TEL-06 conditional on professor approval.

Original scope:

- Grafana setup and provisioned monitoring panels.
- Present throughput, errors, latency, session projections, archive backlog,
  breaker states and discovered/healthy API targets.
- Support approved live telephony testing.

Additional work reported by Chew How Way:

| Work | Implementation and evidence |
| --- | --- |
| Acknowledged-session loss fix | PostgreSQL checkpoints commit before acknowledgment; Redis becomes a best-effort projection. Redis failover rerun: 2,149 lifecycles, no HTTP failures or missing acknowledged session operations |
| Real Groq compatibility | Updated request settings and sentiment response handling. Ten sentiment scores and ten replies succeeded, without audio fallback |
| Queue, alert and archive recovery | Fixed stale claims, shared PostgreSQL alerts and recovery of failed archival |
| Monitoring and inference recovery | Extended HTTP/caller-turn metrics and panels; fixed stale breaker metrics; validated mock outage/restoration |
| Graceful API shutdown | Awaited request draining and added an in-flight-response regression test. Corrected pod-loss run: 2,090 lifecycles, zero HTTP failures, native exit 0 |
| Reliable evaluation tooling | Native process exit capture tested with 0 and 99; exact acknowledged-ID SQL audits |
| Baseline rerun | 1,358 lifecycles, zero HTTP/check failures, native exit 0; completed archives/transcripts verified |
| PostgreSQL recovery tuning | Local smart-shutdown adjustment reduced observed recovery from about 61 to 28 seconds while retaining synchronous durability |
| Regression validation | Recorded 100 Rust tests passed, five ignored; PostgreSQL durability integration passed separately; eight Python checks passed |
| Demo integration | Added Raft mode to the dashboard launcher and rehearsed election, continued escalation and node catch-up |

**What to say:**

> My original responsibility was Grafana observability. During integration,
> I also worked across storage, queueing, inference and shutdown. The largest
> change was moving the acknowledgment boundary to PostgreSQL after observing
> session loss. I repeated the relevant experiments and strengthened independent
> SQL auditing and exit-code recording.
>
> These were cross-cutting fixes on top of the team's components, not a claim
> that I originally implemented every component involved.

### Pieter Imanuel: live telephony support and integration gaps

**Assigned:** support TEL-06 and check for gaps.

- Prepare/check the real Twilio test arrangement and integration prerequisites.
- Check gaps between components, configuration, demonstration and evidence.
- Support consented phone/audio testing once approved.

**What to say:**

> Pieter's assigned role was live telephony support and integration gap
> checking. We distinguish implemented webhook contracts and local simulations
> from an actual end-to-end phone call. Full live Twilio validation remains
> pending the agreed arrangement.

**Do not claim TEL-06 passed:** a JSON dashboard simulation does not validate
real speech, telephone audio or actual outbound volunteer calling.

### Darren Tan Jun Le: coordination and consolidation, subject to confirmation

The supplied allocation message establishes coordination activity but does not
identify its author by name. Attribute the following to Darren only after
confirming that he authored/coordinated that allocation:

- Divide Phase 5 responsibilities and coordinate delivery.
- Seek clarification on the live Twilio arrangement.
- Organize numbers/screenshots for TEST-03 evaluation and DOC-01 reporting.

**Suggested wording once confirmed:**

> Darren coordinated the Phase 5 work split, remaining consultation questions
> and final consolidation of results, screenshots and limitations.

Do not invent additional implementation ownership from this coordination role.

## 3. Test-case results to present as team integration evidence

| Test | What was implemented/exercised | Recorded outcome |
| --- | --- | --- |
| TEST-01 baseline | k6 caller lifecycles plus independent SQL audit | 100 VUs / 30 s; 1,358 lifecycles; zero HTTP failures; HTTP p95 997.87 ms; native exit 0 |
| TEST-02 API pod loss | Delete one API pod under load; verify request drain, service continuity and records | Corrected run: 100 VUs / 60 s; 2,090 lifecycles; zero HTTP failures; replacement Ready 10.112 s; native exit 0 |
| TEST-03 evaluation | Collect metrics, fault timestamps, replica changes, native exits and durable outcome evidence | Results consolidated; not a separate claim that every acceptance gate passed |
| TEST-04 Redis | Automatic Sentinel promotion under traffic; exact-ID durability audit | Initial Redis-first test lost 17 acknowledged creates. After checkpointing: 2,149 lifecycles, zero HTTP failures or missing acknowledged operations |
| TEST-04 PostgreSQL | Primary replacement, recovery observation and SQL audit | Recovery improved 60.995 -> 27.943 s; HTTP failures 2.634% -> 1.600%; acknowledged operations preserved; native exit 99, availability fails |
| TEST-05 HPA | Load plus scaling/error/latency measurements | 100 VUs / 120 s; 3,421 iterations; replicas 3 -> 5 -> 3; 7/13,676 HTTP failures; p95 2,200.26 ms; native exit 99 |
| Real Groq | Ten sequential synthetic sentiment/reply pairs | 10/10 each; no audio fallback; median caller turn 1.13 s, maximum 1.44 s; small integration sample |
| Mock inference recovery | Controlled provider outage/restoration | Recorded 34-lifecycle recovery validation passed; not real-provider latency |
| Raft interactive rehearsal | Gracefully stop leader, elect successor, enqueue after failure, restart/catch up | Node 1 -> node 3 leader; term 1 -> 2; escalation succeeded with 2/3 up; all applied indexes matched after restart |

The Raft rehearsal is not an abrupt-crash load test or independent durability
audit. Kubernetes measurements are synthetic single-node Minikube results,
not evidence of multi-machine/zone resilience. Ignored tests are not passes.
Preserved acknowledged data does not imply uninterrupted availability.

Evidence: [current validation](TEST04_AND_AI_VALIDATION.md),
[dated results](PHASE_5_LOCAL_RESULTS.md),
[Phase 5 milestone mapping](PHASE_5_Windows.md),
[documented earlier ownership](REVIEW_INDEX.md).

## 4. What is Prometheus?

**Plain English:** Prometheus is a monitoring collector and time-series database.
It periodically reads numeric measurements from applications and stores how
those measurements change over time. That periodic read is called a **scrape**.

It does not handle calls, rank callers, elect Raft leaders, restart containers
or itself decide Kubernetes replica counts.

```text
API /api/v1/metrics --scraped by--> Prometheus --queried by--> Grafana charts
                                      |
                              Prometheus Adapter
                                      |
                           request-rate metric for HPA
                                      |
                           Kubernetes scales API pods
```

- Application instrumentation produces measurements.
- Prometheus collects/stores them and supports queries (PromQL).
- Grafana displays charts using those queries.
- The Prometheus Adapter exposes the configured request-rate measurement to
  Kubernetes.
- HPA decides scaling from its configured metrics; Kubernetes applies it.
  CPU/memory HPA metrics are not supplied by this custom request-rate rule.

Analogy: Prometheus records the instruments, Grafana is the display, and HPA
is a controller that acts on selected measurements.

### What our monitoring shows

- HTTP throughput and error rate.
- HTTP p95/p99 latency.
- Active Redis session projections, not authoritative durable-session counts.
- Archive backlog.
- Circuit-breaker states.
- API scrape targets discovered and successfully monitored.

The checked-in local Prometheus configuration uses a one-hour retention window
and disables persistent volume storage. It is local monitoring, not a durable
historical evidence archive. Save experiment reports separately.

## 5. How to see API replicas in Prometheus

### Step 1: use the correct deployment

Our configured Prometheus discovers annotated **Kubernetes API pods**.
It does not automatically scrape the separate Docker Raft nodes on 18081-18083.

For the Raft demonstration, use the application's Raft cluster panel.
For Kubernetes replicas, use kubectl and the Kubernetes monitoring stack.
Running both deployments does not connect their data or monitoring targets.

The application, Prometheus and Grafana must already be installed in Minikube.
See [monitoring setup](GRAFANA_DASHBOARD.md) if they are missing.

### Step 2: open Prometheus

Run this in a separate terminal and leave it open:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
```

Open http://127.0.0.1:19090.

The checked-in values configure the Prometheus Service port as 9090.
If an older installation rejects this forward, inspect its actual ports:

```powershell
kubectl --context minikube -n monitoring get svc prometheus-server
```

Use that Service's port on the right side of the mapping. Do not guess or
change the deployment merely to match the example.

### Step 3: inspect targets and individual scrape health

Open http://127.0.0.1:19090/targets and locate the API targets.
Then enter this expression in Prometheus's query page and execute it:

```promql
up{namespace="sayangcare",pod=~"sayangcare-api-.*"}
```

Each matching series identifies a target using labels such as pod, instance
and job:

- `1`: its latest scrape succeeded.
- `0`: its latest scrape failed.
- No matching data: check discovery, namespace/labels, scrape annotations and
  installation. It is not automatically proof that Kubernetes has zero pods.

### Step 4: count discovered and scrape-healthy targets

These are the queries used by our checked-in Grafana API replicas panel:

**Discovered matching scrape targets:**

```promql
count(up{namespace="sayangcare",pod=~"sayangcare-api-.*"})
```

**Targets whose latest scrape succeeded:**

```promql
sum(up{namespace="sayangcare",pod=~"sayangcare-api-.*"})
```

For the expected single scrape per API pod, values of 3 and 3 mean three
discovered targets and three successful latest scrapes. If they show 3 and 2,
one of the three known targets failed its latest scrape.

**Important:** these count scrape targets, not authoritative Deployment
replicas. Multiple scrape jobs/endpoints can count a pod more than once.
Inspect labels if values look unexpected. During discovery changes, an old
target may disappear rather than remain as `up=0`. Scrape timing and
discovery delay mean the graph need not instantly match kubectl.

### Step 5: verify actual Kubernetes state

```powershell
kubectl --context minikube -n sayangcare get deployment sayangcare-api
kubectl --context minikube -n sayangcare get pods -l app=sayangcare-api
kubectl --context minikube -n sayangcare get hpa sayangcare-api
```

- Deployment output reports desired/current/available rollout state.
- Pod output shows individual pod readiness and status.
- HPA output shows current scaling information and metric targets.
- Prometheus `up` shows monitoring reachability, not full application readiness.
  A successful metrics scrape does not prove PostgreSQL or Redis is ready.

Our local values disable kube-state-metrics, so do not assume metrics such as
`kube_deployment_status_replicas_available` are installed. Use kubectl for the
authoritative replica view in this setup.

### Step 6: show the replica trend

Switch the count/sum query to the graph view and select a time range containing
an actual scaling experiment. If the experiment has aged beyond local
retention or Prometheus restarted, use saved reports/screenshots instead.
Do not expect today's idle cluster to reproduce an earlier 3 -> 5 -> 3 run.

Do not start a disruptive load test solely to make the graph move during a
short consultation. The recorded TEST-05 report contains measured scaling
and cooldown timestamps.

## 6. How to show the same information in Grafana

In another terminal:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

Open http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api.
Find the **API replicas** panel. It charts discovered targets and successful
scrapes using the queries above.

Safe demonstration:

1. Show API pods/Deployment with kubectl.
2. Show the matching Prometheus series.
3. Show Grafana's replica panel.
4. Explain that HPA scaling is a separate controller action, not something
   the monitoring chart itself performs.
5. Show the saved HPA experiment for historical scale-up/down evidence.

Optional safe traffic to the **Kubernetes** dashboard forward:

```powershell
1..10 | ForEach-Object {
    Invoke-WebRequest http://127.0.0.1:18080/api/v1/health | Out-Null
}
```

This requires the Kubernetes API forward on port 18080 to be running.
Wait for subsequent scrapes before inspecting throughput. Ten health requests
demonstrate measurement, not a load benchmark or guaranteed HPA scale-up.

## 7. A one-minute monitoring explanation to say aloud

> Prometheus periodically collects measurements exposed by our API replicas.
> Grafana turns those measurements into charts for traffic, errors, latency,
> session projections, archive backlog, breakers and scrape health.
>
> For replicas, we show both discovered targets and successful scrapes, then
> cross-check Kubernetes Deployment and pod readiness using kubectl. A target
> being scrapeable is not the same as the whole application being ready.
>
> Our request-rate HPA receives a metric through the Prometheus Adapter.
> HPA makes the scaling decision; Prometheus supplies the measurement.
> Our configured monitoring watches Kubernetes, not automatically the separate
> Docker Raft demo. Finally, graphs help explain failures, but our independent
> SQL audits are what check preservation of acknowledged session data.

## 8. Attribution check before presenting

- Confirm actual completed work with each teammate, not only their allocation.
- Credit original implementations and subsequent integration fixes separately.
- Confirm the author/coordinator of the allocation message before naming Darren.
- Keep TEL-06 pending until a real approved phone test is performed.
- Do not describe failed availability/load gates as passes.
- Never display credentials, real caller data or private phone numbers.
