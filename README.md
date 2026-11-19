# SayangCare

SayangCare is a Rust/Actix distributed voice-support **prototype**. The demo
uses synthetic caller sessions, volunteer handoffs, PostgreSQL, Redis and an
optional three-node Raft coordinator. A separate Minikube deployment demonstrates
Kubernetes autoscaling and Prometheus/Grafana monitoring.

> Use synthetic data only. This is not a clinical or emergency service.
> Dashboard roles are not authentication. Never show or commit credentials,
> `.env`, or real caller data. The local demo does not validate live Twilio calls.

## Contents

1. [Setup: Windows](#setup-windows)
2. [Setup: macOS](#setup-macos)
3. [Commands to run: Windows](#commands-to-run-windows)
4. [Commands to run: macOS](#commands-to-run-macos)
5. [Additional information](#additional-information)

## Setup: Windows

These are **first-time setup** instructions for a dedicated local machine and
new Minikube cluster. Stop if a command fails. On an existing/shared cluster,
do not replace its Secrets, operator or Helm releases without its owner's
approval; use [Commands to run: Windows](#commands-to-run-windows) instead.

### 1. Install tools

1. Enable hardware virtualization in BIOS/UEFI. Install
   [WSL 2](https://learn.microsoft.com/windows/wsl/install) from an
   **Administrator PowerShell** and restart Windows when prompted:

   ```powershell
   wsl --install
   ```

2. Install [Docker Desktop for Windows](https://docs.docker.com/desktop/setup/install/windows-install/),
   choose the WSL 2 backend and Linux containers, start Docker Desktop and accept
   its terms. Install [Visual Studio Build Tools](https://visualstudio.microsoft.com/visual-cpp-build-tools/)
   with the **Desktop development with C++** workload for native Rust builds.
3. In PowerShell, install the remaining tools with Windows Package Manager.
   Skip tools already installed:

   ```powershell
   winget install -e --id Git.Git
   winget install -e --id Rustlang.Rustup
   winget install -e --id Kubernetes.minikube
   winget install -e --id Kubernetes.kubectl
   winget install -e --id Helm.Helm
   ```

4. Reopen PowerShell to refresh PATH. If rustup asks for a default toolchain,
   select stable. Check the Docker **server** is running and Rust is 1.88 or
   newer. Keep `kubectl` within one minor version of the Minikube Kubernetes
   server.

   ```powershell
   git --version
   rustc --version
   cargo --version
   docker version
   docker compose version
   minikube version
   kubectl version --client
   helm version
   ```

Allocate sufficient Docker Desktop resources for both deployments: **at least
4 CPUs and 8 GB memory recommended**, plus free disk space for images and
volumes. The first builds and downloads can take several minutes.

### 2. Get the project and configure Compose

```powershell
git clone https://github.com/pieter20037/CSC3104-SayangCare.git
Set-Location CSC3104-SayangCare
if (-not (Test-Path .env)) { Copy-Item .env.example .env }
notepad .env
```

In the private `.env`, uncomment `SAYANGCARE__AI__API_KEY` and set it to your
own Groq key if
you want contextual Groq replies and sentiment. Keep or set
`SAYANGCARE__AI__MODEL` to a model available to your Groq account; the example
uses `openai/gpt-oss-120b`. Without a valid key the app uses deterministic
fallback replies. Groq requests use your quota and send the synthetic
conversation to the provider.

For the local simulator, the example Twilio values are placeholders. Real
Twilio credentials are only needed for a separately configured live integration.
The example PostgreSQL password is **development-only**. If changing it on a
fresh database, update `POSTGRES_PASSWORD`, `DATABASE_URL` and
`SAYANGCARE__POSTGRES__URL` consistently. Existing PostgreSQL volumes retain
their original password; changing `.env` does not update a running database.

### 3. Create the Kubernetes application

Start Docker Desktop first. These commands create local Kubernetes via Minikube,
build and load the API image, and install the CloudNativePG operator. Redis,
Sentinel and PostgreSQL are then created by the Kubernetes manifests; you do
**not** need separate Redis or PostgreSQL installations on Windows.

```powershell
minikube start --profile minikube --driver=docker --cpus=4 --memory=8192
if ($LASTEXITCODE -ne 0) { throw 'Minikube startup failed.' }
minikube addons enable metrics-server --profile minikube
minikube addons enable ingress --profile minikube
kubectl --context minikube get nodes

docker build -t sayangcare-api:dev .
if ($LASTEXITCODE -ne 0) { throw 'API image build failed.' }
minikube image load sayangcare-api:dev --profile minikube
if ($LASTEXITCODE -ne 0) { throw 'Image load failed.' }

kubectl --context minikube apply -f k8s/base/namespace.yaml
kubectl --context minikube -n sayangcare get secrets
```

Only for a **new disposable cluster**, with neither `sayangcare-secrets` nor
`sayangcare-db-app` present, create the following matching local test
credentials. These are not production credentials. Do not overwrite existing
Secrets or show their contents.

```powershell
kubectl --context minikube -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD=local-dev-password --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID=ACtest --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN=test
if ($LASTEXITCODE -ne 0) { throw 'Application Secret creation failed.' }
kubectl --context minikube -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password=local-dev-password
if ($LASTEXITCODE -ne 0) { throw 'Database Secret creation failed.' }

kubectl --context minikube apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
if ($LASTEXITCODE -ne 0) { throw 'CloudNativePG installation failed.' }
kubectl --context minikube -n cnpg-system rollout status deployment/cnpg-controller-manager --timeout=5m
if ($LASTEXITCODE -ne 0) { throw 'CloudNativePG is not ready.' }
kubectl --context minikube apply -k k8s/overlays/minikube
if ($LASTEXITCODE -ne 0) { throw 'Kubernetes deployment failed.' }
kubectl --context minikube -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl --context minikube -n sayangcare rollout status statefulset/redis --timeout=5m
kubectl --context minikube -n sayangcare rollout status statefulset/redis-sentinel --timeout=5m
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=5m
kubectl --context minikube -n sayangcare get pods,svc,pvc,hpa
```

Check each wait/rollout succeeds before continuing. SQL migrations run when
the application starts. The Kubernetes deployment uses its own database and
Redis, independent of the Compose Raft stack.

### 4. Install Prometheus and Grafana

On a new disposable cluster only, check existing releases and Secrets first.
If `grafana-admin` or any listed release exists, stop and inspect the existing
installation rather than overwriting it.

```powershell
kubectl --context minikube create namespace monitoring
kubectl --context minikube -n monitoring get secrets
helm --kube-context minikube list -n monitoring

kubectl --context minikube -n monitoring create secret generic grafana-admin --from-literal=admin-user=admin --from-literal=admin-password=local-demo-only
if ($LASTEXITCODE -ne 0) { throw 'Grafana Secret creation failed.' }
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
helm --kube-context minikube install prometheus prometheus-community/prometheus -n monitoring -f k8s/monitoring/prometheus-values.yaml
if ($LASTEXITCODE -ne 0) { throw 'Prometheus installation failed.' }
helm --kube-context minikube install prometheus-adapter prometheus-community/prometheus-adapter -n monitoring -f k8s/monitoring/prometheus-adapter-values.yaml
if ($LASTEXITCODE -ne 0) { throw 'Prometheus Adapter installation failed.' }
helm --kube-context minikube install grafana grafana/grafana -n monitoring -f k8s/monitoring/grafana-values.yaml
if ($LASTEXITCODE -ne 0) { throw 'Grafana installation failed.' }

kubectl --context minikube -n monitoring rollout status deployment/prometheus-server --timeout=5m
kubectl --context minikube -n monitoring rollout status deployment/prometheus-adapter --timeout=5m
kubectl --context minikube -n monitoring rollout status deployment/grafana --timeout=5m
kubectl --context minikube get apiservice v1beta1.custom.metrics.k8s.io
kubectl --context minikube get --raw /apis/custom.metrics.k8s.io/v1beta1
```

After the adapter API responds and Prometheus has scraped the API pods, check
that the actual request-rate metric is available. If this command reports an
error or no pod values, wait for scraping and retry before applying the overlay:

```powershell
kubectl --context minikube get --raw '/apis/custom.metrics.k8s.io/v1beta1/namespaces/sayangcare/pods/*/sayangcare_http_requests_per_second'
```

Once it returns pod metrics, enable the optional request-rate HPA metric:

```powershell
kubectl --context minikube apply -k k8s/overlays/request-rate-hpa
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
```

If custom metrics remain unavailable, retain the base CPU/memory HPA instead;
an unavailable custom metric can prevent scale-down. Grafana's local anonymous
access is viewer-only. Monitoring storage is not persistent.

## Setup: macOS

These instructions use Terminal and Homebrew on Intel or Apple Silicon. They
describe the equivalent deployment, but **a macOS machine was not available
for end-to-end verification**. On a shared or existing cluster, do not replace
its Secrets, operator or Helm releases; use the recurring commands below.

### 1. Install tools

Install the [Xcode Command Line Tools](https://developer.apple.com/xcode/resources/)
and [Homebrew](https://brew.sh/). Install
[Docker Desktop for Mac](https://docs.docker.com/desktop/setup/install/mac-install/)
for your Intel or Apple Silicon processor, start it and accept its terms.

```bash
xcode-select --install
```

After Homebrew is installed and its `brew` command is on PATH:

```bash
brew install git rustup minikube kubectl helm jq
export PATH="$(brew --prefix rustup)/bin:$PATH"
rustup default stable
open -a Docker
# Wait for Docker Desktop to finish starting before continuing.
git --version
rustc --version
cargo --version
docker version
docker compose version
minikube version
kubectl version --client
helm version
jq --version
```

Homebrew's `rustup` is keg-only; add the `export PATH=...` line above to your
shell startup file (for the default zsh shell, `~/.zprofile`) so it persists
in new Terminal windows.

Use Rust 1.88 or newer and a `kubectl` client within one minor version of the
Minikube server. Allocate at least 4 CPUs and 8 GB memory to Docker Desktop
if available, with enough free disk for images and volumes.

### 2. Get the project and configure Compose

```bash
git clone https://github.com/pieter20037/CSC3104-SayangCare.git
cd CSC3104-SayangCare
test -f .env || cp .env.example .env
open -e .env
```

Privately uncomment and set `SAYANGCARE__AI__API_KEY` and a model available to your Groq
account for live inference. Without the key, deterministic fallback is used.
The example Twilio credentials and PostgreSQL password are for local
simulation only. For a fresh database, keep `POSTGRES_PASSWORD`, `DATABASE_URL`
and `SAYANGCARE__POSTGRES__URL` consistent. Do not overwrite an existing
`.env` or database volume.

### 3. Create the Kubernetes application

The manifests install Redis, Sentinel and PostgreSQL in Kubernetes; no
separate host installations are needed. Start Docker Desktop and run:

```bash
minikube start --profile minikube --driver=docker --cpus=4 --memory=8192
minikube addons enable metrics-server --profile minikube
minikube addons enable ingress --profile minikube
kubectl --context minikube get nodes
docker build -t sayangcare-api:dev .
minikube image load sayangcare-api:dev --profile minikube
kubectl --context minikube apply -f k8s/base/namespace.yaml
kubectl --context minikube -n sayangcare get secrets
```

Stop if any command fails. Only if both application/database Secrets are
absent on a **new disposable cluster**, run:

```bash
kubectl --context minikube -n sayangcare create secret generic sayangcare-secrets --from-literal=POSTGRES_PASSWORD=local-dev-password --from-literal=SAYANGCARE__TELEPHONY__TWILIO_ACCOUNT_SID=ACtest --from-literal=SAYANGCARE__TELEPHONY__TWILIO_AUTH_TOKEN=test
kubectl --context minikube -n sayangcare create secret generic sayangcare-db-app --from-literal=username=sayangcare --from-literal=password=local-dev-password
kubectl --context minikube apply --server-side -f https://raw.githubusercontent.com/cloudnative-pg/cloudnative-pg/release-1.30/releases/cnpg-1.30.1.yaml
kubectl --context minikube -n cnpg-system rollout status deployment/cnpg-controller-manager --timeout=5m
kubectl --context minikube apply -k k8s/overlays/minikube
kubectl --context minikube -n sayangcare wait --for=condition=Ready cluster/sayangcare-db --timeout=10m
kubectl --context minikube -n sayangcare rollout status statefulset/redis --timeout=5m
kubectl --context minikube -n sayangcare rollout status statefulset/redis-sentinel --timeout=5m
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=5m
kubectl --context minikube -n sayangcare get pods,svc,pvc,hpa
```

Check every command succeeds before running the next. Compose and Kubernetes
use separate PostgreSQL and Redis instances.

### 4. Install Prometheus and Grafana

Only for a new disposable cluster:

```bash
kubectl --context minikube create namespace monitoring
kubectl --context minikube -n monitoring get secrets
helm --kube-context minikube list -n monitoring
```

If `grafana-admin` or any of the releases below already exists, stop and inspect
the existing installation. Otherwise:

```bash
kubectl --context minikube -n monitoring create secret generic grafana-admin --from-literal=admin-user=admin --from-literal=admin-password=local-demo-only
helm repo add prometheus-community https://prometheus-community.github.io/helm-charts
helm repo add grafana https://grafana.github.io/helm-charts
helm repo update
helm --kube-context minikube install prometheus prometheus-community/prometheus -n monitoring -f k8s/monitoring/prometheus-values.yaml
helm --kube-context minikube install prometheus-adapter prometheus-community/prometheus-adapter -n monitoring -f k8s/monitoring/prometheus-adapter-values.yaml
helm --kube-context minikube install grafana grafana/grafana -n monitoring -f k8s/monitoring/grafana-values.yaml
kubectl --context minikube -n monitoring rollout status deployment/prometheus-server --timeout=5m
kubectl --context minikube -n monitoring rollout status deployment/prometheus-adapter --timeout=5m
kubectl --context minikube -n monitoring rollout status deployment/grafana --timeout=5m
kubectl --context minikube get apiservice v1beta1.custom.metrics.k8s.io
kubectl --context minikube get --raw /apis/custom.metrics.k8s.io/v1beta1
```

After the adapter API responds and Prometheus has scraped the API pods, check
the actual request-rate metric. If the command reports an error or no pod
values, wait for scraping and retry:

```bash
kubectl --context minikube get --raw '/apis/custom.metrics.k8s.io/v1beta1/namespaces/sayangcare/pods/*/sayangcare_http_requests_per_second'
```

Once it returns pod metrics:

```bash
kubectl --context minikube apply -k k8s/overlays/request-rate-hpa
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
```

If custom metrics are unavailable, retain the base CPU/memory HPA instead.
Grafana and Prometheus use non-persistent local monitoring storage.

## Commands to run: Windows

Run from the repository root in PowerShell, after first-time setup. The first
Raft build is required before using the runbook's `--no-build` startup.
This builds/starts the local Compose PostgreSQL, Redis, Sentinel and three
Raft nodes; it does not deploy the Minikube application. Run each line or
block only after the previous one succeeds; startup and leader election may
take a minute. If a readiness request fails, wait and rerun the checks.

```powershell
cargo build --workspace --locked
if ($LASTEXITCODE -ne 0) { throw 'Workspace build failed.' }
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --build
if ($LASTEXITCODE -ne 0) { throw 'Raft startup failed.' }
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

For subsequent starts with already built images, replace `--build` with
`--no-build` as in the seven-minute runbook. All nodes should report ready,
with one leader and two followers. Groq startup logs say `configured` when a
key was supplied; `not configured` means fallback.

Start Grafana in a separate PowerShell window; keep that window open:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

In the first window, check Grafana and open it:

```powershell
curl.exe --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 --max-time 3 'http://127.0.0.1:13000/api/health'
if ($LASTEXITCODE -ne 0) { throw 'Grafana is not reachable; check the port-forward window.' }
Start-Process 'http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api'
```

Optional Prometheus UI, in another window:

```powershell
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
```

Open `http://127.0.0.1:19090`. Do not start a second forward on a port already
in use; check the existing listener belongs to the intended service.

### Windows demo and evidence

Before recording, test **Start browser voice call** in Caller lab, allow the
microphone, ask an ordinary synthetic question, and verify contextual Groq
response, sentiment and audible playback. End that test call. For fencing,
generate a new synthetic case with **Simulate crisis call**; in **Volunteer
desk**, select Aisha, claim that exact SID, then transfer it once to Noor.
Confirm Noor owns generation 2 under Raft. Stale handoffs can expire after five
minutes; check immediately before the following commands.

```powershell
$base = 'http://127.0.0.1:18081'
$sid = Read-Host 'Paste the synthetic SIM- Call SID'
$current = Invoke-RestMethod "$base/api/v1/calls/$sid/ownership"
$current | Format-List
if ($current.assignment_generation -ne 2 -or $current.volunteer_id -ne 'volunteer-noor' -or $current.coordinator -ne 'raft') {
    throw 'Expected Noor at generation 2 under Raft; prepare and verify a fresh case.'
}
function Send-DemoNote {
    param([string]$Owner, [int]$Generation, [int]$ExpectedStatus)
    $json = @{
        assignment_generation = $Generation
        request_id = [guid]::NewGuid().ToString()
        body = "Synthetic demo note from generation $Generation"
    } | ConvertTo-Json -Compress
    try {
        $response = Invoke-WebRequest -Uri "$base/api/v1/volunteers/$Owner/cases/$sid/notes" -Method Post -ContentType 'application/json' -Body $json -UseBasicParsing
        $status = [int]$response.StatusCode
        $body = $response.Content
    } catch {
        if ($null -eq $_.Exception.Response) { throw }
        $status = [int]$_.Exception.Response.StatusCode
        $body = $_.ErrorDetails.Message
    }
    Write-Host "`n$Owner, generation $Generation - HTTP $status"
    if ($body) { Write-Host $body }
    if ($status -ne $ExpectedStatus) { throw "Expected HTTP $ExpectedStatus, got HTTP $status." }
}
Send-DemoNote -Owner 'volunteer-aisha' -Generation 1 -ExpectedStatus 409
Send-DemoNote -Owner 'volunteer-noor' -Generation 2 -ExpectedStatus 200
```

Print the **recorded**, not live, evaluation results:

```powershell
$r = Get-Content '.\artifacts\phase6-current\final-dependencies\DEP-redis-20261103T084638Z-report.json' -Raw | ConvertFrom-Json
[pscustomobject]@{
    VUs = $r.vus; Duration = $r.duration; Lifecycles = $r.lifecycle_iterations
    HTTPFailurePct = 100 * $r.http_failure_rate; RecoverySeconds = $r.recovery.seconds
    ExitCode = $r.k6_exit_code
} | Format-List
$e = Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-evaluation.json' -Raw | ConvertFrom-Json
$k = Get-Content '.\artifacts\phase6-current\final-hpa\20261103T085504Z-k6-summary.json' -Raw | ConvertFrom-Json
$last = Import-Csv '.\artifacts\phase6-current\final-hpa\20261103T085504Z-hpa-samples.csv' | Select-Object -Last 1
[pscustomobject]@{
    VUs = $e.vus; Load = $e.load_duration; Lifecycles = $k.metrics.iterations.count
    P95ms = [math]::Round($k.metrics.http_req_duration.'p(95)', 1)
    P99ms = [math]::Round($k.metrics.http_req_duration.'p(99)', 1)
    HTTPFailurePct = 100 * $k.metrics.http_req_failed.value
    ReplicaProgression = "$($e.initial_replicas) -> $($e.max_ready_replicas) -> $($last.current_replicas)"
    ExitCode = $e.k6_exit_code
} | Format-List
Start-Process '.\documents\phase6-hpa-replicas.png'
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
$g = Get-Content '.\artifacts\phase5-final\ai\20261025T184504Z-Real\real-ai.json' -Raw | ConvertFrom-Json
$g | Format-List calls,p50_turn_ms,max_turn_ms,audio_fallbacks,validation_passed
```

## Commands to run: macOS

Run from the repository root in Terminal, after first-time setup. Run each
line or block only after the previous one succeeds. The first Raft build is
required before `--no-build` can be used. Startup and leader election may
take a minute; if a readiness request fails, wait and rerun the checks.

```bash
cargo build --workspace --locked
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --build
minikube start --profile minikube --driver=docker
kubectl --context minikube -n sayangcare rollout status deployment/sayangcare-api --timeout=180s
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft ps
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft logs --tail 20 raft-node-1 raft-node-2 raft-node-3 | grep 'Groq inference provider'
for port in 18081 18082 18083; do
  curl --fail --silent --show-error "http://127.0.0.1:$port/api/v1/ready"; echo
  curl --fail --silent --show-error "http://127.0.0.1:$port/api/v1/queue/raft"; echo
done
open 'http://127.0.0.1:18081/dashboard'
open 'http://127.0.0.1:18082/dashboard'
open 'http://127.0.0.1:18083/dashboard'
```

Later starts may use `--no-build` instead of `--build`. All three nodes should
be ready and agree on a leader. `Groq inference provider configured` indicates
the provider was configured; verify a real response separately.

In a second Terminal window, keep the Grafana forward running:

```bash
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/grafana 13000:80
```

In the first window:

```bash
curl --fail --silent --show-error --retry 10 --retry-connrefused --retry-delay 1 --max-time 3 'http://127.0.0.1:13000/api/health'
open 'http://127.0.0.1:13000/d/sayangcare-phase5/sayangcare-api'
```

For Prometheus, keep this running in another Terminal and open
`http://127.0.0.1:19090`:

```bash
kubectl --context minikube -n monitoring port-forward --address 127.0.0.1 svc/prometheus-server 19090:9090
```

### macOS demo and evidence

Use the same browser voice check and fresh Aisha-to-Noor handoff described in
the Windows demo. Once Noor owns generation 2 under Raft, run:

```bash
base='http://127.0.0.1:18081'
printf 'Paste the synthetic SIM- Call SID: '
read -r sid
if ! current=$(curl --fail --silent --show-error "$base/api/v1/calls/$sid/ownership"); then
  echo 'Could not retrieve ownership; no notes sent.'
elif ! printf '%s\n' "$current" | jq -e '.assignment_generation == 2 and .volunteer_id == "volunteer-noor" and .coordinator == "raft"' >/dev/null; then
  printf '%s\n' "$current" | jq .
  echo 'Expected Noor at generation 2 under Raft; prepare and verify a fresh case.'
else
  printf '%s\n' "$current" | jq .
  for generation in 1 2; do
    if [ "$generation" -eq 1 ]; then owner='volunteer-aisha'; expected=409
    else owner='volunteer-noor'; expected=200; fi
    json=$(jq -n --argjson generation "$generation" --arg request_id "$(uuidgen)" \
      '{assignment_generation: $generation, request_id: $request_id, body: ("Synthetic demo note from generation " + ($generation | tostring))}')
    result=$(curl --silent --show-error --write-out '\n%{http_code}' \
      -H 'Content-Type: application/json' -d "$json" \
      "$base/api/v1/volunteers/$owner/cases/$sid/notes") || break
    status=$(printf '%s\n' "$result" | tail -n 1)
    printf '%s, generation %s - HTTP %s\n%s\n' "$owner" "$generation" "$status" "$result"
    if [ "$status" != "$expected" ]; then echo "Expected HTTP $expected; stopping."; break; fi
  done
fi
```

Print recorded results, open the replica chart, and show live Kubernetes status:

```bash
jq '{VUs: .vus, Duration: .duration, Lifecycles: .lifecycle_iterations, HTTPFailurePct: (100 * .http_failure_rate), RecoverySeconds: .recovery.seconds, ExitCode: .k6_exit_code}' artifacts/phase6-current/final-dependencies/DEP-redis-20261103T084638Z-report.json
jq '{VUs: .vus, Load: .load_duration, InitialReplicas: .initial_replicas, MaxReadyReplicas: .max_ready_replicas, ExitCode: .k6_exit_code}' artifacts/phase6-current/final-hpa/20261103T085504Z-evaluation.json
jq '{Lifecycles: .metrics.iterations.count, P95ms: .metrics.http_req_duration["p(95)"], P99ms: .metrics.http_req_duration["p(99)"], HTTPFailurePct: (100 * .metrics.http_req_failed.value)}' artifacts/phase6-current/final-hpa/20261103T085504Z-k6-summary.json
tail -n 1 artifacts/phase6-current/final-hpa/20261103T085504Z-hpa-samples.csv
open documents/phase6-hpa-replicas.png
kubectl --context minikube -n sayangcare get pods,hpa
kubectl --context minikube -n monitoring get pods
jq '{calls, p50_turn_ms, max_turn_ms, audio_fallbacks, validation_passed}' artifacts/phase5-final/ai/20261025T184504Z-Real/real-ai.json
```

The PowerShell-specific scripts and evaluation wrappers in this repository
have not been validated as macOS automation. The commands above are native
Terminal equivalents; the recorded artifacts came from Windows.

## Additional information

### Seven-minute demonstration

The complete timed **Do/Speak** script is in
[Seven-Minute Demo](docs/demo-video/SEVEN_MINUTE_RUNBOOK.md). Its sequence is:

| Time | Show and explain |
| --- | --- |
| 0:00-0:35 | Build, architecture and the durability/ownership goals |
| 0:35-2:10 | Synthetic voice session, Redis hot projection, PostgreSQL checkpoints and atomic archive |
| 2:10-3:20 | Three Raft voters, majority of two and stale/current note responses: 409/200 |
| 3:20-4:35 | Recorded Redis recovery and HPA results; replica chart |
| 4:35-5:35 | Live Minikube pods/HPA, Grafana and circuit breakers |
| 5:35-6:00 | Provider interfaces and extensibility |
| 6:00-7:00 | Twilio/Groq evidence, integration limitations and conclusion |

Use the actual visible status for live Grafana/HPA. `No data` or `<unknown>`
does not mean zero errors or working scaling. Browser speech is not a Twilio
carrier call. All demo notes are synthetic writes.

**0:00-0:35 — Build and architecture.** Show the successful build, then the
architecture. Say:

> SayangCare is a Rust and Actix distributed support prototype. The workspace
> separates API handling, domain logic, storage, queueing, circuit breakers
> and telephony. We address two correctness goals: preserving acknowledged
> session updates through component failure, and rejecting stale volunteer
> writes after reassignment. I will distinguish live behavior from recorded
> experiments. This is a synthetic demonstration, not a validated clinical
> or emergency service.

**0:35-2:10 — Caller workflow and storage.** On node 1, start a fresh browser
voice call, ask for a short story about a friendly robot, show the reply,
transcript, sentiment and version, then end the call. If browser voice is
unavailable, use Start call and Record turn. Say:

> This ordinary conversation demonstrates a contextual Groq reply. Browser
> speech recognition and playback provide the voice interface, not a Twilio
> call.
>
> Follow this synthetic Call SID through the transcript and session version.
> The version makes concurrent updates detectable instead of silently
> overwriting newer state. Redis provides a shared hot projection, and a
> sorted-set queue orders waiting cases by risk and FIFO within a risk level.
>
> However, fast access is not the same as durable acknowledgement.
> PostgreSQL is our commit authority: active sessions have versioned
> checkpoints, while completed transcripts are archived in JSONB. Redis
> Sentinel supports failover, but asynchronous Redis replication alone cannot
> guarantee that the latest acknowledged update survives.
>
> After a durable checkpoint commit, Redis projection has a bounded timeout.
> A projection failure is logged without pretending the PostgreSQL commit
> failed. When finishing a call, the terminal checkpoint and archive are
> written in one database transaction, then hot-state cleanup follows.
> This avoids a crash window between marking a call complete and archiving
> its transcript. The trade-off is deliberate: Redis can lag, while
> PostgreSQL remains the authority for committed session state.

**2:10-3:20 — Raft and fencing.** Show the node links and leader, perform the
fresh Aisha-to-Noor handoff, and run the fencing commands above. Show the
same SID and the 409/200 results. Say:

> Three Raft voters require a majority of two. One voter can fail while
> the remaining majority coordinates ownership; without a majority, new
> ownership decisions cannot safely commit. This does not remove our
> PostgreSQL and Redis dependencies.
>
> Now observe the assignment generation. Aisha's delayed generation-one
> note receives HTTP 409; Noor's generation-two note receives HTTP 200.
> This is fencing: an old client may still be alive, but its authority has
> expired. PostgreSQL checks ownership and generation at the protected write,
> so displaying a new owner in the browser is not the protection itself.
>
> Ownership operations also carry request IDs for idempotent retries.
> A retried operation reuses its ID; an independent note gets a new ID.
> Raft coordinates decisions, while database enforcement rejects stale
> writes. PostgreSQL-only fencing remains supported, so Raft is an optional
> consensus experiment, not a prerequisite for all ownership safety.

**3:20-4:35 — Recorded results.** Show the saved Redis and HPA output and
replica chart. Say:

> These are recorded November third experiments, not faults being injected
> now. With twenty virtual users, the Redis master-pause test completed
> 2,208 lifecycles with zero observed HTTP failures and 8.157 seconds observed
> recovery. The audit also checked acknowledged operations against durable
> records. Recovery time measures the runner's recovery criterion, not a
> promise that every request waited that long.
>
> The hundred-user, 120-second HPA experiment completed 9,248 lifecycles,
> scaling from three to ten replicas and back to three. HTTP p95 was about
> 282 milliseconds and p99 about 507 milliseconds, with zero observed HTTP
> failures. These are request percentiles, not whole-call durations.
> Additional capacity first became Ready after about 62 seconds: scaling
> is not instantaneous. Cooldown took about 503 seconds after load, showing
> the cost-versus-stability trade-off rather than immediate downscaling.

**4:35-5:35 — Kubernetes and monitoring.** Show live pods and HPA, then the
Grafana replicas, latency and circuit-breaker panels. Say:

> This is the separate Minikube deployment, not three autoscaled Raft voters.
> The base HPA targets 65-percent CPU and 75-percent memory utilization,
> bounded between three and twenty API replicas. A separate overlay adds
> request-rate scaling through Prometheus Adapter. Scale-down stabilization
> reduces replica oscillation.
>
> Prometheus collects metrics; Grafana visualizes them. Readiness, latency,
> errors and replica counts answer different questions: Running does not
> automatically mean Ready, and no data does not mean zero errors.
>
> Circuit breakers limit repeated dependency calls: closed admits work,
> open rejects it, and half-open probes recovery. Higher estimated distress
> tightens the adaptive thresholds. Separately, a shared turn deadline reserves
> time for persistence, and a semaphore limits concurrent AI work. These
> mechanisms bound overload; they do not prove clinical safety.

**5:35-6:00 — Extensibility.** Show the architecture. Say:

> Domain-facing storage, inference and telephony interfaces separate workflow
> rules from provider adapters. That makes alternative providers or support
> workflows possible without replacing the ownership protocol. However,
> adapter substitution still requires contract tests, and deployment in a new
> domain requires privacy, security and operational validation.

**6:00-7:00 — Integrations and conclusion.** Show the Twilio limitations,
recorded Groq evidence and conclusion. Say:

> We implemented and locally contract-tested signed Twilio webhooks, TwiML
> and outbound REST integration. Signature validation checks webhook
> authenticity; contract tests do not prove carrier delivery. Our trial used
> a US number, and trial and local-access constraints limited Singapore
> testing. Live inbound conversation and outbound ringing remain unvalidated;
> this is not a universal claim about Twilio trials.
>
> Groq was tested with ten real inference pairs, with recorded median
> caller-turn latency around 1.13 seconds. That is a small integration sample,
> not a production benchmark. High-risk turns bypass generative inference for
> human review. Browser audio demonstrates playback, not a validated carrier
> call. Our contribution is explicit durability, fenced ownership and
> measurable recovery, with integration limits stated rather than hidden.

### Architecture and configuration

- **Compose Raft:** three fixed voters on ports 18081-18083 coordinate
  volunteer ownership. Docker-internal peer addresses are separate from
  browser-facing localhost links. Its PostgreSQL/Redis data is separate from
  Minikube. Compose has one Sentinel with quorum two: it is not an HA
  Sentinel-failover deployment.
- **Minikube:** three API replicas, three Redis nodes, three Sentinels and a
  three-instance CloudNativePG database run on one local Kubernetes node.
  The API uses PostgreSQL-only ownership by default; its replicas are not
  Raft voters. Local single-node operation does not prove machine/zone HA.
- **Storage:** PostgreSQL is the commit authority for versioned checkpoints,
  archived transcripts and ownership fencing. Redis is a hot projection and
  risk/FIFO queue. A stale assignment generation is rejected with HTTP 409.
- **Groq:** Compose explicitly forwards the key, model and base URL from
  `.env`. Changing these requires container recreation, for example:

  ```text
  docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft up -d --no-build --no-deps raft-node-1 raft-node-2 raft-node-3
  ```

  Kubernetes reads its optional Groq key from `sayangcare-secrets`, not the
  host `.env`; do not assume configuring Compose configures Kubernetes.
- **Twilio:** signed webhooks and TwiML were locally contract-tested. Live
  carrier calls require valid credentials, a voice-capable number, public HTTPS
  callbacks and separate validation; the dashboard is a simulation.

### Troubleshooting and stopping

| Symptom | Action |
| --- | --- |
| Docker server unavailable | Start Docker Desktop; retry `docker version`. |
| Raft nodes restarting | Run the Compose `up -d` command above to start dependencies; inspect `docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft logs --tail 50`. |
| Repeated generic AI response | Check Groq configuration and startup logs; confirm the key and model privately, recreate nodes, then test a fresh ordinary turn. Provider failures or timeouts can also trigger fallback. |
| Kubernetes connection refused | Start the existing Minikube profile, then check `minikube status --profile minikube` and pod readiness. |
| Grafana connection refused | Start or restart its port-forward and wait for `/api/health`; opening the URL alone does not start the forward. |
| HPA `<unknown>` | Check Metrics Server, Prometheus Adapter and scraping; allow time for metrics. Do not claim active scaling until metrics are available. |
| Old image or page | Rebuild Compose images, or rebuild/load the Kubernetes image and roll out the API. A source build alone does not update running containers. |
| Port already occupied | Identify the listener and use a free port; do not terminate someone else's process. |

To stop your own port-forwards, press `Ctrl+C` in their terminal windows. To
stop your own local deployments without deleting named volumes:

```text
docker compose -f docker-compose.yml -f docker-compose.raft.yml --profile raft down
minikube stop --profile minikube
```

Do not use `docker compose down -v`, delete PVCs or run `minikube delete`
without the data owner's approval. Back up valuable data before upgrades.

### Evidence and testing

Recorded results and exact definitions are in
[Report metrics traceability](docs/REPORT_METRICS_TRACEABILITY.md); the
[presentation and AI handoff](docs/PRESENTATION_AND_AI_HANDOFF.md) contains
additional experiment details. The Redis and HPA figures are historical
synthetic runs, not guarantees about a current deployment.

Run workspace checks in either platform's terminal:

```text
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --locked
```

Ignored database integration tests need explicitly configured **disposable**
test databases and are not counted as passes. Fault and load tests can disrupt
services: run them only on an approved dedicated environment.
