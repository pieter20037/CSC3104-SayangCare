[CmdletBinding()]
param(
    [ValidateSet('MockRecovery', 'Real')]
    [string]$Mode = 'MockRecovery',
    [string]$Context = 'minikube',
    [string]$Namespace = 'sayangcare',
    [string]$KubectlPath = 'kubectl',
    [string]$PythonPath = '.venv\Scripts\python.exe',
    [string]$Binary = 'target\debug\sayangcare.exe',
    [ValidateRange(1, 20)]
    [int]$RealAiCalls = 3
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..").Path
$python = (Resolve-Path (Join-Path $root $PythonPath)).Path
$api = (Resolve-Path (Join-Path $root $Binary)).Path
$benchmark = Join-Path $PSScriptRoot 'perf-benchmark.py'
$forwards = [System.Collections.Generic.List[object]]::new()
$run = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$output = Join-Path $root "artifacts\phase5-final\ai\$run-$Mode"
if ($Mode -eq 'Real') {
    & $python -B $benchmark --check-real-ai
    if ($LASTEXITCODE -ne 0) { throw 'Live AI configuration is missing or invalid; no provider calls made.' }
}
foreach ($port in @(18190, 18191, 18192)) {
    $probe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $port)
    try { $probe.Start() } finally { $probe.Stop() }
}
New-Item -ItemType Directory -Force -Path $output | Out-Null
function Kube {
    param([string[]]$Arguments)
    $result = & $KubectlPath --context $Context -n $Namespace @Arguments
    if ($LASTEXITCODE -ne 0) { throw "kubectl failed: $($Arguments -join ' ')" }
    return ($result -join "`n")
}
function Forward {
    param([string]$Target, [string]$Mapping)
    $name = $Target.Replace('/', '-')
    $process = Start-Process -FilePath $KubectlPath -ArgumentList @('--context', $Context,
        '-n', $Namespace, 'port-forward', $Target, $Mapping, '--address', '127.0.0.1') `
        -WorkingDirectory $root -PassThru -NoNewWindow `
        -RedirectStandardOutput "$output\$name-forward.log" -RedirectStandardError "$output\$name-forward.err"
    $null = $process.Handle
    $forwards.Add($process)
    $port = [int]($Mapping -split ':')[0]
    $deadline = [DateTime]::UtcNow.AddSeconds(20)
    do {
        if ($process.HasExited) { throw "Forward failed for $Target." }
        $client = [System.Net.Sockets.TcpClient]::new()
        try {
            $connected = $client.ConnectAsync('127.0.0.1', $port)
            if ($connected.Wait(500) -and $client.Connected) { return }
        } catch [System.AggregateException] {
            if ($_.Exception.InnerException -isnot [System.Net.Sockets.SocketException]) { throw }
        } finally { $client.Dispose() }
        Start-Sleep -Milliseconds 200
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Forward for $Target did not listen within 20 seconds."
}
$oldDatabaseUrl = $env:SAYANGCARE__POSTGRES__URL
try {
    $address = Kube -Arguments @('exec', 'redis-sentinel-0', '--', 'redis-cli', '-p', '26379',
        '--raw', 'SENTINEL', 'get-master-addr-by-name', 'sayangcare-master')
    $master = (($address -split "`n")[0] -split '\.')[0]
    if ($master -notmatch '^redis-[0-9]+$') { throw 'Unexpected Redis master; refusing to guess.' }
    Forward -Target "pod/$master" -Mapping '18190:6379'
    Forward -Target 'svc/sayangcare-db-rw' -Mapping '18191:5432'
    $secret = (Kube -Arguments @('get', 'secret', 'sayangcare-db-app', '-o', 'json')) | ConvertFrom-Json
    if ($secret.data.uri) {
        $uri = [System.Text.Encoding]::UTF8.GetString([Convert]::FromBase64String($secret.data.uri))
        $env:SAYANGCARE__POSTGRES__URL = $uri -replace '@[^/]+/', '@127.0.0.1:18191/'
        if ($env:SAYANGCARE__POSTGRES__URL -eq $uri) { throw 'Could not map database URI to the owned local forward.' }
    } elseif ($secret.data.username -and $secret.data.password) {
        $user = [Uri]::EscapeDataString([System.Text.Encoding]::UTF8.GetString(
            [Convert]::FromBase64String($secret.data.username)))
        $password = [Uri]::EscapeDataString([System.Text.Encoding]::UTF8.GetString(
            [Convert]::FromBase64String($secret.data.password)))
        $env:SAYANGCARE__POSTGRES__URL = "postgresql://${user}:${password}@127.0.0.1:18191/sayangcare"
    } else {
        throw 'Database application secret requires uri or username/password fields.'
    }
    $modeFlag = '--breaker-recovery'
    if ($Mode -eq 'Real') { $modeFlag = '--real-ai' }
    & $python -B $benchmark --binary $api --port 18192 --redis-url 'redis://127.0.0.1:18190' `
        --output $output --real-ai-calls $RealAiCalls $modeFlag
    if ($LASTEXITCODE -ne 0) { throw "AI $Mode evaluation failed; inspect local evidence under $output." }
    Write-Host "Evidence: $output"
} finally {
    $env:SAYANGCARE__POSTGRES__URL = $oldDatabaseUrl
    foreach ($process in $forwards) {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force; $process.WaitForExit() }
    }
}
