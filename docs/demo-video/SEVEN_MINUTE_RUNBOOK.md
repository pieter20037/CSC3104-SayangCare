# SayangCare Seven-Minute Demo

## 1. Commands

### Before recording - run from the repository root

```powershell
cargo build --workspace --locked
if ($LASTEXITCODE -ne 0) { throw 'Build failed.' }

docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --no-build
if ($LASTEXITCODE -ne 0) { throw 'Compose startup failed.' }

minikube start --profile minikube --driver=docker
if ($LASTEXITCODE -ne 0) { throw 'Minikube startup failed.' }

kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=180s
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods

docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft ps
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft logs --tail 20 raft-node-1 raft-node-2 raft-node-3 | Select-String 'Groq inference provider'

foreach ($port in 18081..18083) {
    Invoke-RestMethod "http://127.0.0.1:$port/api/v1/ready"
    Invoke-RestMethod "http://127.0.0.1:$port/api/v1/queue/raft"
}

Start-Process 'http://127.0.0.1:18081/dashboard'
Start-Process 'http://127.0.0.1:18082/dashboard'
Start-Process 'http://127.0.0.1:18083/dashboard'
```

### Before recording - start Grafana connection and open it

```powershell
if (-not (Get-NetTCPConnection -LocalPort 13000 -State Listen -ErrorAction SilentlyContinue)) {
    Start-Process powershell.exe -ArgumentList '-NoExit', '-Command', 'kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80'
}
curl.exe --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 --max-time 3 'http://127.0.0.1:13000/api/health'
if ($LASTEXITCODE -ne 0) { throw 'Grafana is not reachable. Check the port-forward window.' }
Start-Process 'http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api'
```

Keep the port-forward PowerShell window open throughout recording.

### Before recording - necessary browser checks

1. In **Caller lab**, generate a fresh Call SID and click **Start browser
   voice call**. Allow the microphone, ask for a short story about a friendly
   robot, and check that the reply is contextual, sentiment is populated and
   the recording captures the spoken audio. Click **End call** afterward.
2. Confirm all three nodes are ready, with one leader and two followers.
   The Groq logs must say **configured**, not **not configured**.
3. Keep the three dashboard tabs, Grafana and a terminal open. Use synthetic
   data only; never show `.env` or credentials.

### 2:10 - fencing demonstration

In **Caller lab**, generate a fresh Call SID, click **Simulate crisis call**,
and copy its SID. In **Volunteer desk**, set **Working as** to **Aisha Rahman**.
Click **Claim** on that exact SID, not **Claim next** on an unrelated case.
Confirm generation 1 in **Assigned cases**, then click **Transfer** once.
Confirm **Noor Hazim**, generation 2 and Raft ownership before running:

```powershell
$base = 'http://127.0.0.1:18081'
$sid = Read-Host 'Paste a fresh synthetic SIM- Call SID assigned to Noor at generation 2'
$current = Invoke-RestMethod "$base/api/v1/calls/$sid/ownership"
$current | Format-List

if ($current.assignment_generation -ne 2 -or
    $current.volunteer_id -ne 'volunteer-noor' -or
    $current.coordinator -ne 'raft') {
    throw 'Expected Noor at generation 2 under Raft. Prepare a fresh case; stale handoffs can expire after five minutes.'
}

function Send-DemoNote {
    param([string]$Owner, [int]$Generation, [int]$ExpectedStatus)

    $json = @{
        assignment_generation = $Generation
        request_id = [guid]::NewGuid().ToString()
        body = "Synthetic demo note from generation $Generation"
    } | ConvertTo-Json -Compress

    try {
        $response = Invoke-WebRequest `
            -Uri "$base/api/v1/volunteers/$Owner/cases/$sid/notes" `
            -Method Post -ContentType 'application/json' `
            -Body $json -UseBasicParsing
        $status = [int]$response.StatusCode
        $body = $response.Content
    } catch {
        if ($null -eq $_.Exception.Response) { throw }
        $status = [int]$_.Exception.Response.StatusCode
        $body = $_.ErrorDetails.Message
    }

    Write-Host "`n$Owner, generation $Generation - HTTP $status"
    if ($body) { Write-Host $body }
    if ($status -ne $ExpectedStatus) {
        throw "Expected HTTP $ExpectedStatus, got HTTP $status."
    }
}

Send-DemoNote -Owner 'volunteer-aisha' -Generation 1 -ExpectedStatus 409
Send-DemoNote -Owner 'volunteer-noor' -Generation 2 -ExpectedStatus 200
```

### 3:20 - recorded Redis failover and HPA results

```powershell
$r = Get-Content '.\artifacts\phase6-current\final-dependencies\DEP-redis-20261103T084638Z-report.json' -Raw | ConvertFrom-Json
[pscustomobject]@{
    VUs = $r.vus
    Duration = $r.duration
    Lifecycles = $r.lifecycle_iterations
    HTTPFailurePct = 100 * $r.http_failure_rate
    RecoverySeconds = $r.recovery.seconds
    ExitCode = $r.k6_exit_code
} | Format-List

$e = Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-evaluation.json' -Raw | ConvertFrom-Json
$k = Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-k6-summary.json' -Raw | ConvertFrom-Json
$last = Import-Csv '.\artifacts\phase6-current\final-hpa\20261103T085504Z-hpa-samples.csv' | Select-Object -Last 1
[pscustomobject]@{
    VUs = $e.vus
    Load = $e.load_duration
    Lifecycles = $k.metrics.iterations.count
    P95ms = [math]::Round($k.metrics.http_req_duration.'p(95)', 1)
    P99ms = [math]::Round($k.metrics.http_req_duration.'p(99)', 1)
    HTTPFailurePct = 100 * $k.metrics.http_req_failed.value
    ReplicaProgression = "$($e.initial_replicas) -> $($e.max_ready_replicas) -> $($last.current_replicas)"
    ExitCode = $e.k6_exit_code
} | Format-List

Start-Process '.\documents\phase6-hpa-replicas.png'
```

### 4:35 - live Kubernetes status

```powershell
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
```

### 6:00 - recorded Groq results

```powershell
$g = Get-Content '.\artifacts\phase5-final\ai\20261025T184504Z-Real\real-ai.json' -Raw | ConvertFrom-Json
$g | Format-List calls,p50_turn_ms,max_turn_ms,audio_fallbacks,validation_passed
```

## 2. Demo actions and spoken script - approximately seven minutes

Read only the **Speak** paragraphs aloud. The **Do** lines are your screen actions.

### 0:00-0:35 | Architecture and purpose | 35 seconds

**Do:** Show the successful build output, then the architecture/title slide.

**Speak:**

SayangCare is a Rust and Actix distributed support prototype. The workspace
separates API handling, domain logic, storage, queueing, circuit breakers
and telephony. We address two correctness goals: preserving acknowledged
session updates through component failure, and rejecting stale volunteer
writes after reassignment. I will distinguish live behavior from recorded
experiments. This is a synthetic demonstration, not a validated clinical
or emergency service.

### 0:35-2:10 | Workflow and hot/cold storage | 95 seconds

**Do:** Open node 1's **Caller lab**. Generate a fresh Call SID, click
**Start browser voice call**, and say: "Tell me a short story about a friendly
robot." Let a brief reply play. Point at the transcript, sentiment and version,
then click **End call**. If voice recognition is unavailable, use **Start
call**, type the same request and click **Record turn** instead.

**Speak:**

This ordinary conversation demonstrates a contextual Groq reply. Browser
speech recognition and playback provide the voice interface, not a Twilio call.

Follow this synthetic Call SID through the transcript and session version.
The version makes concurrent updates detectable instead of silently
overwriting newer state. Redis provides a shared hot projection, and a
sorted-set queue orders waiting cases by risk and FIFO within a risk level.

However, fast access is not the same as durable acknowledgement.
PostgreSQL is our commit authority: active sessions have versioned
checkpoints, while completed transcripts are archived in JSONB. Redis
Sentinel supports failover, but asynchronous Redis replication alone cannot
guarantee that the latest acknowledged update survives.

After a durable checkpoint commit, Redis projection has a bounded timeout.
A projection failure is logged without pretending the PostgreSQL commit
failed. When finishing a call, the terminal checkpoint and archive are
written in one database transaction, then hot-state cleanup follows.
This avoids a crash window between marking a call complete and archiving
its transcript. The trade-off is deliberate: Redis can lag, while
PostgreSQL remains the authority for committed session state.

### 2:10-3:20 | Raft, handoff and fencing | 70 seconds

**Do:** Point at the **Raft cluster** panel and open node 2 using its link.
Show the same leader across nodes. Perform the fresh-case Aisha-to-Noor
handoff above, then run the fencing commands. Keep the SID and HTTP
**409 / 200** results visible. Do not transfer a second time.

**Speak:**

Three Raft voters require a majority of two. One voter can fail while
the remaining majority coordinates ownership; without a majority, new
ownership decisions cannot safely commit. This does not remove our
PostgreSQL and Redis dependencies.

Now observe the assignment generation. Aisha's delayed generation-one
note receives HTTP 409; Noor's generation-two note receives HTTP 200.
This is fencing: an old client may still be alive, but its authority has
expired. PostgreSQL checks ownership and generation at the protected write,
so displaying a new owner in the browser is not the protection itself.

Ownership operations also carry request IDs for idempotent retries.
A retried operation reuses its ID; an independent note gets a new ID.
Raft coordinates decisions, while database enforcement rejects stale
writes. PostgreSQL-only fencing remains supported, so Raft is an optional
consensus experiment, not a prerequisite for all ownership safety.

### 3:20-4:35 | Recovery and scaling results | 75 seconds

**Do:** Run the saved Redis/HPA result commands. Point at recovery time,
failure percentage and p95/p99, then show the replica chart. These commands
read saved results; do not start a live load or failure test.

**Speak:**

These are recorded November third experiments, not faults being injected
now. With twenty virtual users, the Redis master-pause test completed
2,208 lifecycles with zero observed HTTP failures and 8.157 seconds observed
recovery. The audit also checked acknowledged operations against durable
records. Recovery time measures the runner's recovery criterion, not a
promise that every request waited that long.

The hundred-user, 120-second HPA experiment completed 9,248 lifecycles,
scaling from three to ten replicas and back to three. HTTP p95 was about
282 milliseconds and p99 about 507 milliseconds, with zero observed HTTP
failures. These are request percentiles, not whole-call durations.
Additional capacity first became Ready after about 62 seconds: scaling
is not instantaneous. Cooldown took about 503 seconds after load, showing
the cost-versus-stability trade-off rather than immediate downscaling.

### 4:35-5:35 | Kubernetes, monitoring and circuit breakers | 60 seconds

**Do:** Run the live Kubernetes status commands. Switch to Grafana and point
at **API replicas**, latency and **Circuit breakers**. Describe the values
actually visible. If HPA shows `<unknown>`, say the live metric is unavailable;
do not present the historical scaling result as current activity.

**Speak:**

This is the separate Minikube deployment, not three autoscaled Raft voters.
The base HPA targets 65-percent CPU and 75-percent memory utilization,
bounded between three and twenty API replicas. A separate overlay adds
request-rate scaling through Prometheus Adapter. Scale-down stabilization
reduces replica oscillation.

Prometheus collects metrics; Grafana visualizes them. Readiness, latency,
errors and replica counts answer different questions: Running does not
automatically mean Ready, and no data does not mean zero errors.

Circuit breakers limit repeated dependency calls: closed admits work,
open rejects it, and half-open probes recovery. Higher estimated distress
tightens the adaptive thresholds. Separately, a shared turn deadline reserves
time for persistence, and a semaphore limits concurrent AI work. These
mechanisms bound overload; they do not prove clinical safety.

### 5:35-6:00 | Extensibility | 25 seconds

**Do:** Return to the architecture slide and point at the storage, inference
and telephony interfaces.

**Speak:**

Domain-facing storage, inference and telephony interfaces separate workflow
rules from provider adapters. That makes alternative providers or support
workflows possible without replacing the ownership protocol. However,
adapter substitution still requires contract tests, and deployment in a new
domain requires privacy, security and operational validation.

### 6:00-7:00 | Twilio, Groq and conclusion | 60 seconds

**Do:** Show the Twilio integration/limitations slide, run the recorded Groq
results command, then finish on the conclusion slide.

**Speak:**

We implemented and locally contract-tested signed Twilio webhooks, TwiML
and outbound REST integration. Signature validation checks webhook
authenticity; contract tests do not prove carrier delivery. Our trial used
a US number, and trial and local-access constraints limited Singapore
testing. Live inbound conversation and outbound ringing remain unvalidated;
this is not a universal claim about Twilio trials.

Groq was tested with ten real inference pairs, with recorded median
caller-turn latency around 1.13 seconds. That is a small integration sample,
not a production benchmark. High-risk turns bypass generative inference for
human review. Browser audio demonstrates playback, not a validated carrier
call. Our contribution is explicit durability, fenced ownership and
measurable recovery, with integration limits stated rather than hidden.
