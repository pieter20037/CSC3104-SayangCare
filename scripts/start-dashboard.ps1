[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)][int]$Port = 18080,
    [ValidateRange(10, 900)][int]$TimeoutSeconds = 120,
    [switch]$SkipMinikubeStart,
    [switch]$OpenBrowser,
    [switch]$CheckOnly
)

$ErrorActionPreference = 'Stop'
$forward = $null
$client = $null
$handler = $null
$logPrefix = Join-Path ([IO.Path]::GetTempPath()) ("sayangcare-forward-" + [guid]::NewGuid())
$stdout = "$logPrefix.out"
$stderr = "$logPrefix.err"
$boot = [char]::ConvertFromUtf32(0x1F331)
$good = [char]::ConvertFromUtf32(0x2705)
$bad = [char]::ConvertFromUtf32(0x274C)
$heart = [char]::ConvertFromUtf32(0x1F49A)

function Wait-ClusterResource {
    param([string]$Label, [string[]]$KubectlArgs)
    Write-Host "`n$boot $Label" -ForegroundColor Cyan
    & $script:kubectl --context minikube @KubectlArgs
    if ($LASTEXITCODE -ne 0) {
        throw "$Label failed. Check the resource with kubectl --context minikube."
    }
    Write-Host "$good $Label complete." -ForegroundColor Green
}

function Stop-ConnectionProcess {
    param([int]$ProcessId)
    # Chocolatey may launch kubectl through a shim; close that connection's children too.
    $children = Get-CimInstance Win32_Process -Filter "ParentProcessId = $ProcessId"
    foreach ($child in $children) {
        Stop-ConnectionProcess -ProcessId $child.ProcessId
    }
    $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -ne $process -and -not $process.HasExited) {
        Stop-Process -Id $ProcessId
        $process.WaitForExit()
    }
}

try {
    Write-Host "`n$heart SayangCare is booting. Let's get your care workspace ready!" -ForegroundColor Cyan
    $kubectl = (Get-Command kubectl -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
    if (-not $SkipMinikubeStart) {
        $minikubeCommand = Get-Command minikube -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($null -ne $minikubeCommand) {
            $minikube = $minikubeCommand.Source
        }
        else {
            $defaultMinikube = Join-Path $env:ProgramFiles 'Kubernetes\Minikube\minikube.exe'
            if (Test-Path -LiteralPath $defaultMinikube -PathType Leaf) {
                $minikube = $defaultMinikube
            }
            else {
                throw "Minikube was not found on PATH or at its default install location ($defaultMinikube). Reopen VS Code after installing Minikube, or add its install folder to PATH."
            }
        }
        & $minikube start --profile minikube --driver=docker
        if ($LASTEXITCODE -ne 0) {
            throw 'Minikube could not start. Check that Docker Desktop is running.'
        }
        Write-Host "$good Minikube is awake!" -ForegroundColor Green
    }

    $listener = [Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, $Port)
    try {
        $listener.Start()
    }
    catch [Net.Sockets.SocketException] {
        throw "Port $Port is already in use. Keep your existing forward, or rerun with -Port 18081."
    }
    finally {
        $listener.Stop()
    }

    Wait-ClusterResource 'Checking database operator' @('-n', 'cnpg-system', 'rollout', 'status', 'deployment/cnpg-controller-manager', '--timeout=120s')
    Wait-ClusterResource 'Waiting for PostgreSQL' @('-n', 'sayangcare', 'wait', '--for=condition=Ready', 'cluster/sayangcare-db', '--timeout=10m')
    Wait-ClusterResource 'Waiting for Redis' @('-n', 'sayangcare', 'rollout', 'status', 'statefulset/redis', '--timeout=180s')
    Wait-ClusterResource 'Waiting for Redis Sentinel' @('-n', 'sayangcare', 'rollout', 'status', 'statefulset/redis-sentinel', '--timeout=180s')
    Wait-ClusterResource 'Waiting for the care API' @('-n', 'sayangcare', 'rollout', 'status', 'deployment/sayangcare-api', '--timeout=180s')
    Wait-ClusterResource 'Waiting for ready API pods' @('-n', 'sayangcare', 'wait', '--for=condition=Ready', 'pod', '-l', 'app=sayangcare-api', '--timeout=180s')

    Write-Host "`n$boot Opening your local dashboard connection..." -ForegroundColor Cyan
    Add-Type -AssemblyName System.Net.Http
    $handler = [Net.Http.HttpClientHandler]::new()
    $handler.UseProxy = $false
    $client = [Net.Http.HttpClient]::new($handler)
    $client.Timeout = [TimeSpan]::FromSeconds(3)
    $baseUrl = "http://127.0.0.1:$Port"
    $deadline = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    $ready = $false
    $lastIssue = 'Waiting for the port-forward.'
    while ([DateTime]::UtcNow -lt $deadline) {
        if ($null -eq $forward) {
            foreach ($log in @($stdout, $stderr)) {
                if (Test-Path -LiteralPath $log) {
                    Remove-Item -LiteralPath $log
                }
            }
            $forward = Start-Process -FilePath $kubectl -ArgumentList @(
                '--context', 'minikube', '-n', 'sayangcare', 'port-forward',
                '--address', '127.0.0.1', 'svc/sayangcare-api', "${Port}:80"
            ) -PassThru -NoNewWindow -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        }
        if ($forward.HasExited) {
            $lastIssue = if (Test-Path -LiteralPath $stderr) { Get-Content $stderr -Raw } else { 'The port-forward exited before the API became reachable.' }
            $forward.Dispose()
            $forward = $null
            Start-Sleep -Seconds 2
            continue
        }
        Write-Progress -Activity 'SayangCare is booting' -Status 'Checking API, storage, and dashboard...'
        try {
            foreach ($path in @('/api/v1/health', '/api/v1/ready', '/dashboard')) {
                $response = $client.GetAsync("$baseUrl$path").GetAwaiter().GetResult()
                try {
                    $response.EnsureSuccessStatusCode() | Out-Null
                    $body = $response.Content.ReadAsStringAsync().GetAwaiter().GetResult()
                    if ($path -eq '/api/v1/health') {
                        $health = $body | ConvertFrom-Json
                        if ($health.status -ne 'ok' -or $health.service -ne 'sayangcare') {
                            throw 'The health endpoint did not identify a healthy SayangCare API.'
                        }
                    }
                    elseif ($path -eq '/api/v1/ready') {
                        $storage = $body | ConvertFrom-Json
                        if ($storage.ready -ne $true -or $storage.dependencies.redis -ne 'ok' -or $storage.dependencies.postgres -ne 'ok') {
                            throw 'The API is reachable, but Redis/PostgreSQL are not ready.'
                        }
                    }
                    elseif ($body -notmatch 'SayangCare' -or $body -notmatch 'id="role-layout"') {
                        throw 'The dashboard endpoint did not return the expected workspace.'
                    }
                }
                finally {
                    $response.Dispose()
                }
            }
            $ready = $true
            break
        }
        catch [Net.Http.HttpRequestException] {
            $lastIssue = $_.Exception.Message
        }
        catch [Threading.Tasks.TaskCanceledException] {
            $lastIssue = 'An API check timed out.'
        }
        if ($forward.HasExited) {
            $forward.Dispose()
            $forward = $null
            Start-Sleep -Seconds 2
            continue
        }
        Start-Sleep -Seconds 1
    }
    Write-Progress -Activity 'SayangCare is booting' -Completed
    if (-not $ready) {
        throw "Dashboard was not ready within $TimeoutSeconds seconds. Last check: $lastIssue"
    }

    Write-Host "`n$good API healthy. Redis ready. PostgreSQL ready. Dashboard ready!" -ForegroundColor Green
    if ($CheckOnly) {
        Write-Host "$good Startup check passed. Check-only mode will now close its temporary connection." -ForegroundColor Green
    }
    else {
        Write-Host "`n$heart ACCESS THE DASHBOARD HERE: $baseUrl/dashboard" -ForegroundColor Cyan
        Write-Host 'Ctrl+click the link in VS Code, or copy it into your browser.'
        Write-Host 'Keep this terminal open. Press Ctrl+C to close only this dashboard connection.'
        if ($OpenBrowser) {
            Start-Process "$baseUrl/dashboard"
        }
        while (-not $forward.HasExited) {
            Start-Sleep -Seconds 1
        }
        throw "The dashboard connection ended. Rerun this script to reconnect. $(Get-Content $stderr -Raw)"
    }
}
catch {
    Write-Host "`n$bad SayangCare could not finish booting: $($_.Exception.Message)" -ForegroundColor Red
    throw
}
finally {
    Write-Progress -Activity 'SayangCare is booting' -Completed
    if ($null -ne $client) { $client.Dispose() }
    if ($null -ne $handler) { $handler.Dispose() }
    if ($null -ne $forward -and -not $forward.HasExited) {
        Stop-ConnectionProcess -ProcessId $forward.Id
        $forward.WaitForExit()
    }
    if ($null -ne $forward) { $forward.Dispose() }
    foreach ($log in @($stdout, $stderr)) {
        for ($attempt = 0; $attempt -lt 5 -and (Test-Path -LiteralPath $log); $attempt++) {
            try {
                Remove-Item -LiteralPath $log
            }
            catch [IO.IOException] {
                if ($attempt -eq 4) {
                    Write-Warning "Could not remove temporary connection log ${log}: $($_.Exception.Message)"
                }
                else {
                    Start-Sleep -Milliseconds 200
                }
            }
        }
    }
}
