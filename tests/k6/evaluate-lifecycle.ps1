[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [ValidateSet('baseline', 'pod-failover')]
    [string]$Mode = 'baseline',
    [string]$Context = 'minikube',
    [string]$Namespace = 'sayangcare',
    [ValidateRange(1, 1000)]
    [int]$Vus = 100,
    [string]$Duration = '60s',
    [ValidateRange(1024, 65535)]
    [int]$Port = 18186,
    [ValidateRange(1, 300)]
    [int]$FaultAfterSeconds = 10,
    [string]$KubectlPath = 'kubectl',
    [string]$K6Path = 'k6',
    [string]$OutputDirectory = 'artifacts\phase5-final\lifecycle-checkpoint'
)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'recorded-process.ps1')
if (-not $PSCmdlet.ShouldProcess("$Context/$Namespace", "Run $Mode with $Vus VUs for $Duration; pod-failover deletes one API pod only")) {
    return
}
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$output = Join-Path $root $OutputDirectory
New-Item -ItemType Directory -Force -Path $output | Out-Null
$run = "LIFE-$Mode-" + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$prefix = @('--context', $Context, '-n', $Namespace)
$forward = $null
$recording = $null
$report = [ordered]@{
    run_id = $run; mode = $Mode; vus = $Vus; duration = $Duration
    k6_exit_code = $null; fault = $null; replacement_ready_seconds = $null
    audit = $null; error = $null
}
function Kube {
    param([string[]]$Arguments)
    $text = & $KubectlPath @prefix @Arguments
    if ($LASTEXITCODE -ne 0) { throw "kubectl failed: $($Arguments -join ' ')" }
    return ($text -join "`n")
}
try {
    $deployment = (Kube -Arguments @('get', 'deployment', 'sayangcare-api', '-o', 'json')) | ConvertFrom-Json
    if ($deployment.status.readyReplicas -lt 3) { throw 'Require at least three ready API replicas.' }
    $pods = (Kube -Arguments @('get', 'pods', '-l', 'app=sayangcare-api', '-o', 'json')) | ConvertFrom-Json
    $serving = @($pods.items | Where-Object {
        -not $_.metadata.deletionTimestamp -and $_.status.containerStatuses `
            -and $_.status.containerStatuses[0].ready
    })
    $report.image_ids = @($serving.status.containerStatuses.imageID | Sort-Object -Unique)
    $victim = $serving[0]
    $portProbe = [System.Net.Sockets.TcpListener]::new([System.Net.IPAddress]::Loopback, $Port)
    try { $portProbe.Start() } finally { $portProbe.Stop() }
    $forward = Start-Process -FilePath $KubectlPath -ArgumentList @('--context', $Context,
        '-n', 'ingress-nginx', 'port-forward', 'svc/ingress-nginx-controller',
        "${Port}:80", '--address', '127.0.0.1') -PassThru -NoNewWindow `
        -RedirectStandardOutput "$output\$run-forward.log" -RedirectStandardError "$output\$run-forward.err"
    $null = $forward.Handle
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $code = & curl.exe --noproxy '*' -s -o NUL -w '%{http_code}' --max-time 3 "http://127.0.0.1:$Port/api/v1/ready"
        if ($LASTEXITCODE -eq 0 -and $code -eq '200') { break }
        if ($forward.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Ingress readiness preflight failed.' }
        Start-Sleep -Milliseconds 500
    } while ($true)
    $summaryPath = "$output\$run-k6.json"
    $recording = Start-RecordedProcess -FilePath $K6Path -WorkingDirectory $root `
        -Arguments @('run', '-e', "BASE_URL=http://127.0.0.1:$Port", '-e', "VUS=$Vus",
            '-e', "DURATION=$Duration", '-e', "RUN_ID=$run", '-e', 'RECORD_SESSION_IDS=true',
            '--summary-export', "`"$summaryPath`"", "`"$PSScriptRoot\call-lifecycle.js`"") `
        -StdoutPath "$output\$run-k6.log" -StderrPath "$output\$run-k6.err"
    if ($Mode -eq 'pod-failover') {
        Start-Sleep -Seconds $FaultAfterSeconds
        if ($recording.Process.HasExited) { throw 'Load ended before fault injection.' }
        $watch = [System.Diagnostics.Stopwatch]::StartNew()
        $report.fault = @{
            utc = [DateTime]::UtcNow.ToString('o')
            pod = $victim.metadata.name; uid = $victim.metadata.uid
        }
        Kube -Arguments @('delete', 'pod', $victim.metadata.name, '--wait=false') | Out-Null
        $deadline = [DateTime]::UtcNow.AddSeconds(180)
        do {
            $current = (Kube -Arguments @('get', 'pods', '-l', 'app=sayangcare-api', '-o', 'json')) | ConvertFrom-Json
            $ready = @($current.items | Where-Object {
                -not $_.metadata.deletionTimestamp -and $_.status.containerStatuses `
                    -and $_.status.containerStatuses[0].ready
            })
            $new = @($ready | Where-Object { $_.metadata.uid -notin $pods.items.metadata.uid })
            $desired = ((Kube -Arguments @('get', 'deployment', 'sayangcare-api', '-o', 'json')) | ConvertFrom-Json).spec.replicas
            if ($new.Count -gt 0 -and $ready.Count -ge $desired `
                -and $victim.metadata.uid -notin $ready.metadata.uid) {
                $report.replacement_ready_seconds = [math]::Round($watch.Elapsed.TotalSeconds, 3)
                $report.replacement_uid = $new[0].metadata.uid
                break
            }
            Start-Sleep -Seconds 2
        } while ([DateTime]::UtcNow -lt $deadline)
    }
    $report.k6_exit_code = Complete-RecordedProcess $recording
    $recording = $null
    $summary = Get-Content -Raw $summaryPath | ConvertFrom-Json
    $report.iterations = $summary.metrics.iterations.count
    $report.http_failure_rate = $summary.metrics.http_req_failed.value
    $report.lifecycle_success_rate = $summary.metrics.call_lifecycle_success.value
    $report.http_p95_ms = $summary.metrics.http_req_duration.'p(95)'
    $report.http_p99_ms = $summary.metrics.http_req_duration.'p(99)'
    $primary = ((Kube -Arguments @('get', 'cluster', 'sayangcare-db', '-o', 'json')) | ConvertFrom-Json).status.currentPrimary
    $sql = @"
SELECT json_build_object(
 'rows',count(*),'distinct_ids',count(DISTINCT id),
 'invalid_transcripts',count(*) FILTER (WHERE jsonb_array_length(transcript->'turns') IS DISTINCT FROM 2
 OR transcript->'turns'->0->>'speaker' IS DISTINCT FROM 'caller'
 OR transcript->'turns'->0->>'text' IS DISTINCT FROM 'I have been feeling overwhelmed by a busy day.'
 OR transcript->'turns'->1->>'speaker' IS DISTINCT FROM 'assistant'),
 'nonterminal',count(*) FILTER (WHERE state <> 'completed'),
 'checkpoints',(SELECT count(*) FROM session_checkpoints WHERE starts_with(id,'SIM-K6-$run-')),
 'pending',(SELECT count(*) FROM session_checkpoints WHERE starts_with(id,'SIM-K6-$run-') AND NOT archived))
FROM sessions WHERE starts_with(id,'SIM-K6-$run-');
"@
    $report.audit = (Kube -Arguments @('exec', $primary, '-c', 'postgres', '--', 'psql',
        '-U', 'postgres', '-d', 'sayangcare', '-At', '-c', $sql)) | ConvertFrom-Json
    $report.consistency_passed = ($report.audit.rows -eq $report.iterations `
        -and $report.audit.distinct_ids -eq $report.iterations `
        -and $report.audit.checkpoints -eq $report.iterations `
        -and $report.audit.invalid_transcripts -eq 0 -and $report.audit.nonterminal -eq 0 `
        -and $report.audit.pending -eq 0)
    if ($report.k6_exit_code -ne 0) { throw "k6 thresholds failed: exit $($report.k6_exit_code)." }
    if (-not $report.consistency_passed) { throw 'Independent SQL archive/checkpoint audit failed.' }
    if ($Mode -eq 'pod-failover' -and $null -eq $report.replacement_ready_seconds) {
        throw 'No ready replacement observed within 180 seconds.'
    }
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    if ($null -ne $recording) {
        if (-not $recording.Process.HasExited) { Stop-Process -Id $recording.Process.Id -Force }
        $report.k6_exit_code = Complete-RecordedProcess $recording
    }
    if ($null -ne $forward -and -not $forward.HasExited) {
        Stop-Process -Id $forward.Id -Force
        $forward.WaitForExit()
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content "$output\$run-report.json"
    Write-Host "Evidence: $output\$run-report.json"
}
