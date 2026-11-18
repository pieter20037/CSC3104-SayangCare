# SayangCare Seven-Minute Demo Video Runbook

Use this runbook to prepare and record a seven-minute project demonstration.
It covers compilation, the synthetic workflow, fencing-token HTTP responses,
Redis/PostgreSQL hot-and-cold storage, Minikube/HPA, Prometheus/Grafana, and
the Twilio/Groq attempts.

> Use synthetic data only. SayangCare is a prototype, not a clinical or
> emergency service. Do not show credentials, real caller data, or `.env`.

## Before recording

- **Reminder: run the AI voice/audio trial in the browser first.** Check
  microphone permission if needed, play the available AI or fallback audio,
  and confirm the sound is audible in the recording. Browser playback is not
  proof of Twilio carrier audio. If only text inference is available, describe
  it as text inference rather than voice generation.
- Run the commands from the repository root.
- Compile before recording; the first Rust build can take several minutes.
  Do not let compilation consume the seven-minute presentation:

  ```powershell
  cargo build --workspace --locked
  ```

- Check the two demo environments separately:

  ```powershell
  docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft ps
  minikube status --profile minikube
  kubectl --context minikube -n sayangcare get pods,hpa
  kubectl --context minikube -n monitoring get pods
  ```

- If the Raft stack is not running, start it **before recording**:

  ```powershell
  .\scripts\start-dashboard.ps1 -Mode Raft -OpenBrowser
  ```

- If Grafana's port-forward is not already active, run this in another
  PowerShell window and leave it open:

  ```powershell
  kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
  ```

  Open `http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api`.
  Do not start a second forward if port 13000 is already in use.

### Keep the environments distinct

| What you show | Environment | Address / evidence |
| --- | --- | --- |
| Three-node Raft dashboard and fenced-note demo | Docker Compose | `http://127.0.0.1:18081/dashboard` |
| HPA, Kubernetes pods, Prometheus and Grafana | Minikube | `http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api` |
| HPA replica chart | Recorded experiment, 3 Nov 2026 | [phase6-hpa-replicas.png](../../documents/phase6-hpa-replicas.png) |

These are separate deployments. Minikube's default ownership mode is
PostgreSQL-only; its autoscaled API pods are not the three Raft voters. The
recorded HPA and Redis-failover metrics are historical results, not
measurements from the current Grafana screenshot.

## Seven-minute sequence and narration

### 1. Compilation and project purpose — 0:00–0:35

**Show:** A successful `cargo build --workspace --locked` result, then the
title or architecture slide.

**Say:**

> “SayangCare is a distributed voice-support prototype. We focus on two
> problems: retaining session state when components fail, and preventing a
> previous volunteer from writing to a case after it has been reassigned.
> This is a synthetic demonstration, not a validated clinical or emergency
> service.”

The compile proves the checked-out Rust workspace builds. It does not by
itself prove that an already-running container was built from that checkout.

### 2. Workflow and hot/cold session storage — 0:35–2:10

**Show:** Open the Raft dashboard. In **Caller lab**, show a synthetic
session's Call SID, transcript and version. If practical, record an ordinary
turn and end the call. Use a fresh synthetic case for escalation; do not use
real personal or health information.

**Say:**

> “Redis is our fast, shared hot projection and orders waiting cases by risk
> and FIFO. Redis Sentinel monitors Redis availability, but asynchronous
> replication by itself is not our durability guarantee. PostgreSQL is the
> commit authority: it stores versioned active checkpoints and archives
> completed transcripts in JSONB. Redis is the fast view; PostgreSQL preserves
> the durable state.”

Show the Kubernetes component list if you need to point out the deployed Redis
and Sentinel pods:

```powershell
kubectl --context minikube -n sayangcare get pods,statefulsets
```

### 3. Volunteer handoff, fencing token and HTTP results — 2:10–3:20

**Show:** In **Volunteer desk**, show the intended case assigned to Noor at
generation 2. Show the terminal output with Aisha's stale generation-1 request
returning **HTTP 409** and Noor's current generation-2 request returning
**HTTP 200**.

The last verified demo Call SID was
`SIM-F19F9EDD6BCF4EFDB879958C0B53985D`. Check ownership before using it; do
not click **Transfer** again if it is already at generation 2.

For a live rerun, use the working PowerShell `Send-DemoNote` block from this
runbook. It validates ownership first and uses `Invoke-WebRequest` so the
expected 409 is displayed instead of treated as a script failure:

```powershell
$base = 'http://127.0.0.1:18081'
$sid = Read-Host 'Paste the current synthetic SIM- Call SID'
$current = Invoke-RestMethod "$base/api/v1/calls/$sid/ownership"
$current | Format-List

if ($current.assignment_generation -ne 2 -or
    $current.volunteer_id -ne 'volunteer-noor' -or
    $current.coordinator -ne 'raft') {
    throw 'Expected Noor at generation 2 under Raft. Stop and check the case.'
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

    Write-Host "`n$Owner, generation $Generation — HTTP $status"
    if ($body) { Write-Host $body }
    if ($status -ne $ExpectedStatus) {
        throw "Expected HTTP $ExpectedStatus, got HTTP $status."
    }
}

Send-DemoNote -Owner 'volunteer-aisha' -Generation 1 -ExpectedStatus 409
Send-DemoNote -Owner 'volunteer-noor' -Generation 2 -ExpectedStatus 200
```

**Say:**

> “The assignment generation is a fencing token. A delayed request from
> generation 1 gets HTTP 409, while the current owner's generation-2 request
> gets HTTP 200. Raft coordinates ownership decisions through a majority;
> PostgreSQL enforces the generation at the protected write. PostgreSQL can
> also enforce fencing without Raft, so Raft is an optional consensus
> experiment, not a claim that PostgreSQL cannot protect ownership.”

If the ownership guard fails, stop; do not send the notes. Do not claim that
the 409/200 proves a carrier call occurred.

### 4. Show metrics: Redis failover and HPA — 3:20–4:35

**Show:** Run these read-only commands to print the saved evaluation results.
Then show the HPA chart. Do not start a load or fault test during the video.

Redis failover result:

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
```

**Say:** “In the recorded Minikube Redis master-pause test, 2,208 lifecycles
completed, HTTP failure rate was zero, and observed recovery was 8.157 seconds.
This is the saved test result, not a fault I am triggering now.”

HPA result:

```powershell
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
```

**Say:** “The recorded 100-VU, 120-second HPA run completed 9,248 lifecycles:
replicas scaled 3 to 10 and back to 3, with zero HTTP failures, p95 about
282 milliseconds and p99 about 507 milliseconds. This chart is from the
November 3 evaluation, not the current live load.”

### 5. Minikube, Prometheus, Grafana and circuit breaker — 4:35–5:35

**Show:** Run `kubectl --context minikube -n sayangcare get pods,hpa`, then
switch to Grafana. Point out the **Circuit breakers** panel and **API
replicas** panel.

**Say:**

> “Minikube runs the Kubernetes application components and HPA. Prometheus
> scrapes the API metrics; Grafana presents throughput, latency, session and
> breaker panels. In this snapshot, three API targets are discovered and
> healthy, and both breakers are currently closed. ‘No data’ on the error
> panel does not mean zero errors. This live, low-load snapshot is not the
> historical HPA benchmark.”

### 6. Extensibility — 5:35–6:00

**Show:** Architecture or conclusion slide.

**Say:**

> “The same distributed-system patterns could be adapted for other support
> workflows, such as social services, appointment follow-up or non-emergency
> hotlines. Each domain would still need its own privacy, security,
> operational and service-specific validation.”

### 7. Finish with Twilio and Groq attempts — 6:00–7:00

**Show:** Twilio limitation slide, then Groq evidence or the final slide.

**Say:**

> “We implemented and locally contract-tested signed Twilio webhooks, TwiML
> and outbound REST integration. Our trial setup used a US number, and trial
> and local-access constraints limited live Singapore testing. Our approved
> demonstration is dashboard-based; live inbound conversation and outbound
> ringing remain unvalidated. This is our experience, not a claim that all
> Twilio trial accounts only provide US numbers.”
>
> “We also tested Groq with ten real inference pairs; the recorded median
> caller-turn latency was about 1.13 seconds. Groq is optional, and high-risk
> turns are routed to human review rather than generative inference. The
> browser audio trial, if shown, demonstrates browser playback only—not a
> validated Twilio call.”

Optional read-only Groq evidence command:

```powershell
$g = Get-Content '.\artifacts\phase5-final\ai\20261025T184504Z-Real\real-ai.json' -Raw | ConvertFrom-Json
$g | Format-List calls,p50_turn_ms,max_turn_ms,audio_fallbacks,validation_passed
```

## Safety and timing notes

- Compile, start services, open browser tabs and verify audio **before**
  recording. Do not let builds, port-forward setup or Kubernetes startup run
  inside the seven-minute segment.
- Do not run `minikube delete`, `docker compose down -v`, live k6 load,
  Redis-pause, pod-kill or Raft leader-stop tests during this recording.
- Use the same synthetic Call SID across the ownership screen, HTTP responses
  and any audit evidence. The note requests above are synthetic writes.
- Keep the Redis and HPA figures labeled as recorded experiments. Grafana is a
  separate live snapshot.
- For source paths, artifact locations and metric definitions, see
  [REPORT_METRICS_TRACEABILITY.md](../REPORT_METRICS_TRACEABILITY.md) and
  the project [README](../../README.md).
