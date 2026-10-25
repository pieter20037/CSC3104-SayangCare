[CmdletBinding()]
param(
    [switch]$OpenBrowser,
    [ValidateRange(30, 900)][int]$TimeoutSeconds = 300,
    [ValidateRange(1024, 65500)][int]$ApiPort = 18080,
    [ValidateRange(1024, 65500)][int]$GrafanaPort = 13000,
    [ValidateRange(1024, 65500)][int]$PrometheusPort = 19090
)

$ErrorActionPreference = 'Stop'
$forwards = [System.Collections.Generic.List[object]]::new()
$tempLogs = [System.Collections.Generic.List[string]]::new()
$httpHandler = $null
$httpClient = $null
$repoRoot = $null
$locationPushed = $false

function Resolve-Executable {
    param(
        [Parameter(Mandatory)][string]$Name,
        [string[]]$Candidates = @()
    )

    $command = Get-Command $Name -CommandType Application -ErrorAction SilentlyContinue |
        Select-Object -First 1
    if ($null -ne $command) {
        return $command.Source
    }

    foreach ($candidate in $Candidates) {
        if (Test-Path -LiteralPath $candidate -PathType Leaf) {
            return $candidate
        }
    }

    throw "$Name was not found. Install it and reopen PowerShell, or add it to PATH."
}

function Test-PortAvailable {
    param([int]$Port)

    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
    try {
        $listener.Start()
        return $true
    }
    catch [Net.Sockets.SocketException] {
        return $false
    }
    finally {
        $listener.Stop()
    }
}

function Select-FreePort {
    param([int]$PreferredPort)

    for ($candidate = $PreferredPort; $candidate -le [Math]::Min(65500, $PreferredPort + 50); $candidate++) {
        if (Test-PortAvailable -Port $candidate) {
            if ($candidate -ne $PreferredPort) {
                Write-Host "Port $PreferredPort is busy; using $candidate instead." -ForegroundColor Yellow
            }
            return $candidate
        }
    }

    throw "No available port found near $PreferredPort."
}

function Invoke-Kubectl {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & $script:kubectlPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "kubectl failed: kubectl $($Arguments -join ' ')"
    }
}

function Invoke-Helm {
    param([Parameter(Mandatory)][string[]]$Arguments)

    & $script:helmPath @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "Helm failed: helm $($Arguments -join ' ')"
    }
}

function Start-PortForward {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Namespace,
        [Parameter(Mandatory)][string]$Service,
        [Parameter(Mandatory)][int]$LocalPort,
        [Parameter(Mandatory)][int]$ServicePort
    )

    $stdout = Join-Path ([IO.Path]::GetTempPath()) ("sayangcare-$Name-" + [guid]::NewGuid() + '.out')
    $stderr = Join-Path ([IO.Path]::GetTempPath()) ("sayangcare-$Name-" + [guid]::NewGuid() + '.err')
    $tempLogs.Add($stdout)
    $tempLogs.Add($stderr)
    $arguments = @(
        '--context', 'minikube', '-n', $Namespace, 'port-forward',
        '--address', '127.0.0.1', $Service, "${LocalPort}:$ServicePort"
    )
    $process = Start-Process -FilePath $script:kubectlPath -ArgumentList $arguments `
        -PassThru -WindowStyle Hidden -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $forwards.Add([pscustomobject]@{
        Name = $Name
        Process = $process
        Stdout = $stdout
        Stderr = $stderr
    })
}

function Get-HttpText {
    param([Parameter(Mandatory)][string]$Url)

    $response = $script:httpClient.GetAsync($Url).GetAwaiter().GetResult()
    try {
        if (-not $response.IsSuccessStatusCode) {
            throw "HTTP $([int]$response.StatusCode) from $Url"
        }
        return $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
    }
    finally {
        $response.Dispose()
    }
}

function Wait-HttpText {
    param(
        [Parameter(Mandatory)][string]$Url,
        [Parameter(Mandatory)][string]$Description,
        [Parameter(Mandatory)][DateTime]$Deadline
    )

    $lastError = 'Waiting for the service to answer.'
    while ([DateTime]::UtcNow -lt $Deadline) {
        foreach ($forward in $forwards) {
            if ($forward.Process.HasExited) {
                $detail = if (Test-Path -LiteralPath $forward.Stderr) {
                    Get-Content -LiteralPath $forward.Stderr -Raw
                }
                else {
                    'No port-forward error output was recorded.'
                }
                throw "$($forward.Name) port-forward exited: $detail"
            }
        }

        try {
            $body = Get-HttpText -Url $Url
            Write-Host "Ready: $Description" -ForegroundColor Green
            return $body
        }
        catch {
            $lastError = $_.Exception.Message
            Start-Sleep -Seconds 2
        }
    }

    throw "$Description did not become ready. Last check: $lastError"
}

try {
    $repoRoot = Split-Path -Parent $PSScriptRoot
    Push-Location $repoRoot
    $locationPushed = $true
    Write-Host 'Starting SayangCare local observability stack...' -ForegroundColor Cyan

    $script:kubectlPath = Resolve-Executable -Name 'kubectl' -Candidates @(
        (Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\kubectl.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\resources\bin\kubectl.exe')
    )
    $script:minikubePath = Resolve-Executable -Name 'minikube' -Candidates @(
        (Join-Path $env:ProgramFiles 'Kubernetes\Minikube\minikube.exe')
    )
    $helmCandidates = @(
        (Join-Path $env:ProgramFiles 'Helm\helm.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\Helm\helm.exe')
    )
    $wingetHelmRoot = Join-Path $env:LOCALAPPDATA 'Microsoft\WinGet\Packages'
    if (Test-Path -LiteralPath $wingetHelmRoot -PathType Container) {
        $helmCandidates += @(Get-ChildItem -Path (Join-Path $wingetHelmRoot 'Helm.Helm*') `
            -Filter 'helm.exe' -Recurse -ErrorAction SilentlyContinue |
            Select-Object -ExpandProperty FullName)
    }
    $script:helmPath = Resolve-Executable -Name 'helm' -Candidates $helmCandidates
    $dockerPath = Resolve-Executable -Name 'docker' -Candidates @(
        (Join-Path $env:ProgramFiles 'Docker\Docker\resources\bin\docker.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\resources\bin\docker.exe')
    )

    $dockerDesktop = @(
        (Join-Path $env:ProgramFiles 'Docker\Docker\Docker Desktop.exe'),
        (Join-Path $env:LOCALAPPDATA 'Programs\DockerDesktop\Docker Desktop.exe')
    ) | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1

    $dockerReady = $false
    & $dockerPath info --format '{{.ServerVersion}}' 2>$null | Out-Null
    $dockerReady = $LASTEXITCODE -eq 0
    if (-not $dockerReady) {
        if (-not $dockerDesktop) {
            throw 'Docker Desktop is not running and its application was not found. Start Docker Desktop, wait for the engine, then rerun this script.'
        }
        Write-Host 'Launching Docker Desktop and waiting for its engine...' -ForegroundColor Yellow
        Start-Process -FilePath $dockerDesktop | Out-Null
        $dockerDeadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
        while ([DateTime]::UtcNow -lt $dockerDeadline) {
            Start-Sleep -Seconds 3
            & $dockerPath info --format '{{.ServerVersion}}' 2>$null | Out-Null
            if ($LASTEXITCODE -eq 0) {
                $dockerReady = $true
                break
            }
        }
        if (-not $dockerReady) {
            throw "Docker Desktop did not become ready within $TimeoutSeconds seconds."
        }
    }
    Write-Host 'Ready: Docker engine' -ForegroundColor Green

    $minikubeStatus = & $script:minikubePath status --profile minikube 2>&1 | Out-String
    if ($LASTEXITCODE -ne 0 -and $minikubeStatus -match 'Profile .*not found') {
        throw 'The Minikube profile is missing. Complete the one-time setup in docs/PHASE_4_KUBERNETES.md, then rerun this launcher.'
    }
    if ($minikubeStatus -notmatch '(?im)^host:\s+Running\s*$') {
        Write-Host 'Starting the existing Minikube profile...' -ForegroundColor Cyan
        & $script:minikubePath start --profile minikube --driver=docker
        if ($LASTEXITCODE -ne 0) {
            throw 'Minikube could not start. Check Docker Desktop and minikube status.'
        }
    }
    Invoke-Kubectl -Arguments @('--context', 'minikube', 'wait', '--for=condition=Ready', 'node', '--all', '--timeout=180s')

    $apiDeployment = & $script:kubectlPath --context minikube -n sayangcare get deployment sayangcare-api -o name 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $apiDeployment) {
        throw 'The Phase 4 SayangCare application is not installed in this cluster. Complete its one-time setup in docs/PHASE_4_KUBERNETES.md first.'
    }

    Write-Host 'Waiting for the Phase 4 application...' -ForegroundColor Cyan
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'cnpg-system', 'rollout', 'status', 'deployment/cnpg-controller-manager', '--timeout=180s')
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'sayangcare', 'wait', '--for=condition=Ready', 'cluster/sayangcare-db', '--timeout=10m')
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'sayangcare', 'rollout', 'status', 'statefulset/redis', '--timeout=180s')
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'sayangcare', 'rollout', 'status', 'statefulset/redis-sentinel', '--timeout=180s')
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'sayangcare', 'rollout', 'status', 'deployment/sayangcare-api', '--timeout=180s')

    Write-Host 'Installing/updating Prometheus, Adapter, and Grafana...' -ForegroundColor Cyan
    Invoke-Helm -Arguments @('repo', 'add', 'prometheus-community', 'https://prometheus-community.github.io/helm-charts', '--force-update')
    Invoke-Helm -Arguments @('repo', 'add', 'grafana', 'https://grafana.github.io/helm-charts', '--force-update')
    Invoke-Helm -Arguments @('repo', 'update')
    Invoke-Helm -Arguments @('upgrade', '--install', 'prometheus', 'prometheus-community/prometheus', '--namespace', 'monitoring', '--create-namespace', '--values', 'k8s/monitoring/prometheus-values.yaml')
    Invoke-Helm -Arguments @('upgrade', '--install', 'prometheus-adapter', 'prometheus-community/prometheus-adapter', '--namespace', 'monitoring', '--values', 'k8s/monitoring/prometheus-adapter-values.yaml')

    $grafanaSecret = & $script:kubectlPath --context minikube -n monitoring get secret grafana-admin --ignore-not-found -o name 2>$null
    if ($LASTEXITCODE -ne 0) {
        throw 'Could not check the Grafana admin secret.'
    }
    if (-not $grafanaSecret) {
        $bytes = New-Object byte[] 32
        $rng = [Security.Cryptography.RandomNumberGenerator]::Create()
        try {
            $rng.GetBytes($bytes)
        }
        finally {
            $rng.Dispose()
        }
        $password = [Convert]::ToBase64String($bytes)
        Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'monitoring', 'create', 'secret', 'generic', 'grafana-admin', '--from-literal=admin-user=admin', "--from-literal=admin-password=$password")
    }
    Invoke-Helm -Arguments @('upgrade', '--install', 'grafana', 'grafana/grafana', '--namespace', 'monitoring', '--values', 'k8s/monitoring/grafana-values.yaml')

    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'monitoring', 'rollout', 'status', 'deployment/prometheus-server', "--timeout=${TimeoutSeconds}s")
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'monitoring', 'rollout', 'status', 'deployment/prometheus-adapter', "--timeout=${TimeoutSeconds}s")
    Invoke-Kubectl -Arguments @('--context', 'minikube', '-n', 'monitoring', 'rollout', 'status', 'deployment/grafana', "--timeout=${TimeoutSeconds}s")

    $apiPort = Select-FreePort -PreferredPort $ApiPort
    $grafanaPort = Select-FreePort -PreferredPort $GrafanaPort
    $prometheusPort = Select-FreePort -PreferredPort $PrometheusPort
    Start-PortForward -Name 'api' -Namespace 'sayangcare' -Service 'svc/sayangcare-api' -LocalPort $apiPort -ServicePort 80
    Start-PortForward -Name 'grafana' -Namespace 'monitoring' -Service 'svc/grafana' -LocalPort $grafanaPort -ServicePort 80
    Start-PortForward -Name 'prometheus' -Namespace 'monitoring' -Service 'svc/prometheus-server' -LocalPort $prometheusPort -ServicePort 9090

    Add-Type -AssemblyName System.Net.Http
    $httpHandler = [Net.Http.HttpClientHandler]::new()
    $httpHandler.UseProxy = $false
    $httpClient = [Net.Http.HttpClient]::new($httpHandler)
    $httpClient.Timeout = [TimeSpan]::FromSeconds(8)
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)

    $apiBase = "http://127.0.0.1:$apiPort"
    $grafanaBase = "http://127.0.0.1:$grafanaPort"
    $prometheusBase = "http://127.0.0.1:$prometheusPort"
    $health = (Wait-HttpText -Url "$apiBase/api/v1/ready" -Description 'SayangCare API and storage' -Deadline $deadline) | ConvertFrom-Json
    if ($health.ready -ne $true) {
        throw 'The API is reachable, but PostgreSQL or Redis is not ready.'
    }
    $metrics = Wait-HttpText -Url "$apiBase/api/v1/metrics" -Description 'API Prometheus metrics' -Deadline $deadline
    if ($metrics -notmatch 'sayangcare_http_requests_by_route_total') {
        throw 'The API metrics endpoint did not expose the expected route request counter.'
    }

    $null = Wait-HttpText -Url "$grafanaBase/api/health" -Description 'Grafana' -Deadline $deadline
    $dashboard = (Wait-HttpText -Url "$grafanaBase/api/dashboards/uid/sayangcare-phase5" -Description 'Provisioned SayangCare dashboard' -Deadline $deadline) | ConvertFrom-Json
    if (@($dashboard.dashboard.panels).Count -ne 7) {
        throw "Expected seven dashboard panels; found $(@($dashboard.dashboard.panels).Count)."
    }
    $targetsJson = (Wait-HttpText -Url "$prometheusBase/api/v1/targets" -Description 'Prometheus' -Deadline $deadline) | ConvertFrom-Json
    $apiTargets = @($targetsJson.data.activeTargets | Where-Object {
        $_.labels.namespace -eq 'sayangcare' -and $_.labels.pod -like 'sayangcare-api-*'
    })
    if ($apiTargets.Count -eq 0 -or @($apiTargets | Where-Object { $_.health -ne 'up' }).Count -gt 0) {
        throw 'Prometheus has no healthy SayangCare API scrape targets.'
    }
    $datasourceQuery = (Wait-HttpText -Url "$grafanaBase/api/datasources/proxy/uid/prometheus/api/v1/query?query=up" -Description 'Grafana to Prometheus connection' -Deadline $deadline) | ConvertFrom-Json
    if ($datasourceQuery.status -ne 'success' -or @($datasourceQuery.data.result).Count -eq 0) {
        throw 'Grafana could not query Prometheus.'
    }

    1..5 | ForEach-Object { $null = Get-HttpText -Url "$apiBase/api/v1/health" }
    Write-Host "`nAll services are ready." -ForegroundColor Green
    Write-Host "Call Lab:    $apiBase/dashboard"
    Write-Host "Grafana:     $grafanaBase/d/sayangcare-phase5/sayangcare-api"
    Write-Host "Prometheus:  $prometheusBase"
    Write-Host "API targets: $($apiTargets.Count) healthy; dashboard panels: $(@($dashboard.dashboard.panels).Count)"
    Write-Host 'A few health requests were sent to create fresh throughput samples; allow a scrape interval for the charts to update.'
    if ($OpenBrowser) {
        Start-Process "$grafanaBase/d/sayangcare-phase5/sayangcare-api"
    }
    Write-Host "`nKeep this terminal open while using the dashboards. Press Ctrl+C to close the temporary port-forwards." -ForegroundColor Cyan
    while ($true) {
        foreach ($forward in $forwards) {
            if ($forward.Process.HasExited) {
                $detail = if (Test-Path -LiteralPath $forward.Stderr) { Get-Content -LiteralPath $forward.Stderr -Raw } else { 'No port-forward error output was recorded.' }
                throw "$($forward.Name) port-forward exited: $detail"
            }
        }
        Start-Sleep -Seconds 2
    }
}
catch {
    Write-Host "`nOne-shot startup failed: $($_.Exception.Message)" -ForegroundColor Red
    throw
}
finally {
    foreach ($forward in $forwards) {
        if (-not $forward.Process.HasExited) {
            Stop-Process -Id $forward.Process.Id -Force -ErrorAction SilentlyContinue
        }
        $forward.Process.Dispose()
    }
    if ($null -ne $httpClient) { $httpClient.Dispose() }
    if ($null -ne $httpHandler) { $httpHandler.Dispose() }
    foreach ($log in $tempLogs) {
        Remove-Item -LiteralPath $log -Force -ErrorAction SilentlyContinue
    }
    if ($locationPushed) {
        Pop-Location
    }
}
