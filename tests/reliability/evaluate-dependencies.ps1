[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('postgres', 'redis')]
    [string]$Dependency,
    [ValidateSet('pod-replacement', 'sentinel-failover', 'master-pause')]
    [string]$FaultMode = 'pod-replacement',
    [string]$Context = 'minikube',
    [string]$Namespace = 'sayangcare',
    [int]$Port = 18185,
    [int]$Vus = 30,
    [string]$Duration = '60s',
    [int]$RecoveryTimeoutSeconds = 180,
    [ValidateRange(30000, 120000)]
    [int]$PauseMilliseconds = 60000,
    [switch]$AllowTransientFailures,
    [string]$OutputDirectory = 'artifacts\phase5-final\dependencies',
    [string]$KubectlPath = 'kubectl',
    [string]$K6Path = 'k6'
)

$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot '..\k6\recorded-process.ps1')
if ($Port -lt 1024 -or $Port -gt 65535 -or $Vus -lt 1 -or $RecoveryTimeoutSeconds -lt 30) {
    throw 'Port must be 1024..65535, Vus positive, and recovery timeout at least 30 seconds.'
}
if ($FaultMode -ne 'pod-replacement' -and $Dependency -ne 'redis') {
    throw 'sentinel-failover and master-pause are only valid for Redis.'
}
if ($FaultMode -eq 'master-pause' -and $RecoveryTimeoutSeconds * 1000 -le $PauseMilliseconds) {
    throw 'Recovery timeout must exceed the bounded Redis pause.'
}
if (-not $PSCmdlet.ShouldProcess("$Context/$Namespace", "Run $FaultMode on the $Dependency primary during synthetic traffic; preserve PVCs")) {
    return
}
$root = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$output = Join-Path $root $OutputDirectory
New-Item -ItemType Directory -Force -Path $output | Out-Null
$run = 'DEP-{0}-{1}' -f $Dependency, [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$prefix = @('--context', $Context, '-n', $Namespace)
$forward = $null
$load = $null
$recording = $null
$report = [ordered]@{
    run_id = $run; dependency = $Dependency; fault_mode = $FaultMode; context = $Context; namespace = $Namespace
    vus = $Vus; duration = $Duration; fault = $null; recovery = $null
    audit = $null; error = $null; k6_exit_code = $null; samples = @()
}
function Kube {
    param([string[]]$Arguments)
    $text = & $KubectlPath @prefix @Arguments
    if ($LASTEXITCODE -ne 0) { throw "kubectl failed ($LASTEXITCODE): $($Arguments -join ' ')" }
    return ($text -join "`n")
}
function RedisMaster {
    $address = Kube -Arguments @('exec', 'redis-sentinel-0', '--', 'redis-cli', '-p', '26379',
        '--raw', 'SENTINEL', 'get-master-addr-by-name', 'sayangcare-master')
    $hostName = ($address -split "`n")[0].Trim()
    if ($hostName -notmatch '^redis-[0-9]+\.redis\.') {
        throw "Unexpected Sentinel master address: $address"
    }
    return ($hostName -split '\.')[0]
}
function Primary {
    if ($Dependency -eq 'redis') { return (RedisMaster) }
    $cluster = (Kube -Arguments @('get', 'cluster', 'sayangcare-db', '-o', 'json')) | ConvertFrom-Json
    return $cluster.status.currentPrimary
}
function Ready {
    $code = & curl.exe --noproxy '*' -s -o NUL -w '%{http_code}' --max-time 5 "http://127.0.0.1:$Port/api/v1/ready"
    return ($LASTEXITCODE -eq 0 -and $code -eq '200')
}
try {
    if ($Dependency -eq 'postgres') {
        $preflight = (Kube -Arguments @('get', 'cluster', 'sayangcare-db', '-o', 'json')) | ConvertFrom-Json
        if ($preflight.status.readyInstances -lt 3) { throw 'Require three ready database instances before a fault.' }
    } else {
        foreach ($set in @('redis', 'redis-sentinel')) {
            $preflight = (Kube -Arguments @('get', 'statefulset', $set, '-o', 'json')) | ConvertFrom-Json
            if ($preflight.status.readyReplicas -lt 3) { throw "Require three ready $set pods before a fault." }
        }
    }
    $before = Primary
    if ([string]::IsNullOrWhiteSpace($before)) { throw 'No current primary reported.' }
    $beforePod = (Kube -Arguments @('get', 'pod', $before, '-o', 'json')) | ConvertFrom-Json
    $report.primary_before = $before
    $report.primary_uid_before = $beforePod.metadata.uid
    $forward = Start-Process -FilePath $KubectlPath -ArgumentList @('--context', $Context,
        '-n', 'ingress-nginx', 'port-forward', 'svc/ingress-nginx-controller',
        "${Port}:80", '--address', '127.0.0.1') -PassThru -NoNewWindow `
        -RedirectStandardOutput "$output\$run-forward.log" -RedirectStandardError "$output\$run-forward.err"
    $null = $forward.Handle
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while (-not (Ready)) {
        if ($forward.HasExited -or [DateTime]::UtcNow -gt $deadline) { throw 'Ingress readiness failed before load.' }
        Start-Sleep -Seconds 1
    }
    $summaryPath = "$output\$run-k6.json"
    $arguments = @('run', '-e', "BASE_URL=http://127.0.0.1:$Port", '-e', "VUS=$Vus",
        '-e', "DURATION=$Duration", '-e', "RUN_ID=$run", '-e', 'RECORD_SESSION_IDS=true',
        '--summary-export', "`"$summaryPath`"", "`"$root\tests\k6\call-lifecycle.js`"")
    $recording = Start-RecordedProcess -FilePath $K6Path -Arguments $arguments `
        -WorkingDirectory $root -StdoutPath "$output\$run-k6.log" -StderrPath "$output\$run-k6.err"
    $load = $recording.Process
    Start-Sleep -Seconds 10
    if ($load.HasExited) { throw 'Load ended before fault injection.' }
    $watch = [System.Diagnostics.Stopwatch]::StartNew()
    $report.fault = @{ utc = [DateTime]::UtcNow.ToString('o'); primary = $before }
    if ($FaultMode -eq 'master-pause') {
        $report.fault.pause_ms = $PauseMilliseconds
        $reply = Kube -Arguments @('exec', $before, '--', 'redis-cli',
            'CLIENT', 'PAUSE', "$PauseMilliseconds", 'ALL')
        if ($reply.Trim() -ne 'OK') { throw "Redis rejected bounded pause: $reply" }
    } elseif ($FaultMode -eq 'sentinel-failover') {
        $reply = Kube -Arguments @('exec', 'redis-sentinel-0', '--', 'redis-cli', '-p', '26379',
            'SENTINEL', 'FAILOVER', 'sayangcare-master')
        if ($reply.Trim() -ne 'OK') { throw "Sentinel rejected failover: $reply" }
    } else {
        Kube -Arguments @('delete', 'pod', $before, '--wait=false') | Out-Null
    }
    $recoveryDeadline = [DateTime]::UtcNow.AddSeconds($RecoveryTimeoutSeconds)
    do {
        $current = Primary
        $healthy = Ready
        $report.samples += @{
            utc = [DateTime]::UtcNow.ToString('o'); primary = $current; ready = $healthy
        }
        if ($current -ne $before -and $healthy) {
            $report.recovery = @{
                utc = [DateTime]::UtcNow.ToString('o'); primary = $current
                seconds = [math]::Round($watch.Elapsed.TotalSeconds, 3)
            }
            break
        }
        Start-Sleep -Seconds 2
    } while ([DateTime]::UtcNow -lt $recoveryDeadline)
    $report.primary_change_observed = ($null -ne $report.recovery)
    if (-not $report.primary_change_observed) {
        $report.recovery_limitation = 'No different healthy primary observed; primary promotion is not demonstrated by this run.'
    }
    $report.k6_exit_code = Complete-RecordedProcess $recording
    $recording = $null
    $load = $null
    $summary = Get-Content -Raw -LiteralPath $summaryPath | ConvertFrom-Json
    $report.lifecycle_iterations = $summary.metrics.iterations.count
    $report.lifecycle_success_rate = $summary.metrics.call_lifecycle_success.value
    $report.http_failure_rate = $summary.metrics.http_req_failed.value
    $report.http_p95_ms = $summary.metrics.http_req_duration.'p(95)'
    $report.http_p99_ms = $summary.metrics.http_req_duration.'p(99)'
    $acknowledged = @(Get-Content -LiteralPath @("$output\$run-k6.log", "$output\$run-k6.err") | ForEach-Object {
        if ($_ -match 'ACKNOWLEDGED_SESSION (SIM-K6-[A-Za-z0-9-]+)') { $Matches[1] }
    } | Sort-Object -Unique)
    $report.acknowledged_session_count = $acknowledged.Count
    $turnAcknowledged = @(Get-Content -LiteralPath @("$output\$run-k6.log", "$output\$run-k6.err") | ForEach-Object {
        if ($_ -match 'ACKNOWLEDGED_TURN (SIM-K6-[A-Za-z0-9-]+)') { $Matches[1] }
    } | Sort-Object -Unique)
    $hangupAcknowledged = @(Get-Content -LiteralPath @("$output\$run-k6.log", "$output\$run-k6.err") | ForEach-Object {
        if ($_ -match 'ACKNOWLEDGED_HANGUP (SIM-K6-[A-Za-z0-9-]+)') { $Matches[1] }
    } | Sort-Object -Unique)
    $report.acknowledged_turn_count = $turnAcknowledged.Count
    $report.acknowledged_hangup_count = $hangupAcknowledged.Count
    $expectedAcknowledged = $summary.root_group.checks.'incoming response contains the generated session id'.passes
    if ($acknowledged.Count -ne $expectedAcknowledged) {
        throw 'Acknowledged-session log count does not match k6 checks; SQL audit cannot establish ID coverage.'
    }
    if ($turnAcknowledged.Count -ne $summary.root_group.checks.'turn response contains an assistant reply'.passes `
        -or $hangupAcknowledged.Count -ne $summary.root_group.checks.'hangup response is terminal'.passes) {
        throw 'Acknowledged turn/hangup log counts do not match k6 checks.'
    }

    # Give terminal-session recovery a sweep interval before reading the primary.
    Start-Sleep -Seconds 35
    $cluster = (Kube -Arguments @('get', 'cluster', 'sayangcare-db', '-o', 'json')) | ConvertFrom-Json
    $dbPrimary = $cluster.status.currentPrimary
    $sql = @"
SELECT json_build_object(
 'rows', count(*),
 'ids', coalesce(json_agg(id), '[]'::json),
 'distinct_ids', count(DISTINCT id),
 'invalid_transcripts', count(*) FILTER (WHERE jsonb_array_length(transcript->'turns') <> 2
   OR transcript->'turns'->0->>'speaker' <> 'caller'
   OR transcript->'turns'->0->>'text' <> 'I have been feeling overwhelmed by a busy day.'
   OR transcript->'turns'->1->>'speaker' <> 'assistant'),
 'nonterminal', count(*) FILTER (WHERE state NOT IN ('completed','abandoned','failed','escalated')),
 'min_version', min(version), 'max_version', max(version))
FROM sessions WHERE starts_with(id, 'SIM-K6-$run-');
"@
    $audit = Kube -Arguments @('exec', $dbPrimary, '-c', 'postgres', '--', 'psql',
        '-U', 'postgres', '-d', 'sayangcare', '-At', '-c', $sql)
    $report.audit = $audit | ConvertFrom-Json
    $report.missing_acknowledged_ids = @($acknowledged | Where-Object { $_ -notin $report.audit.ids })
    $report.archived_all_acknowledged = ($acknowledged.Count -gt 0 `
        -and $report.missing_acknowledged_ids.Count -eq 0)
    $checkpointSql = @"
SELECT json_build_object(
 'rows', count(*),
 'ids', coalesce(json_agg(id), '[]'::json),
 'valid_turn_ids', coalesce(json_agg(id) FILTER (WHERE jsonb_array_length(snapshot->'transcript'->'turns')=2
   AND snapshot->'transcript'->'turns'->0->>'speaker'='caller'
   AND snapshot->'transcript'->'turns'->0->>'text'='I have been feeling overwhelmed by a busy day.'
   AND snapshot->'transcript'->'turns'->1->>'speaker'='assistant'), '[]'::json),
 'pending', count(*) FILTER (WHERE NOT archived))
FROM session_checkpoints WHERE starts_with(id, 'SIM-K6-$run-');
"@
    $report.durable_audit = (Kube -Arguments @('exec', $dbPrimary, '-c', 'postgres', '--', 'psql',
        '-U', 'postgres', '-d', 'sayangcare', '-At', '-c', $checkpointSql)) | ConvertFrom-Json
    $report.missing_durable_ids = @($acknowledged | Where-Object { $_ -notin $report.durable_audit.ids })
    $report.invalid_acknowledged_turn_ids = @($turnAcknowledged | Where-Object { $_ -notin $report.durable_audit.valid_turn_ids })
    $report.missing_acknowledged_archive_ids = @($hangupAcknowledged | Where-Object { $_ -notin $report.audit.ids })
    $report.durable_all_acknowledged = ($acknowledged.Count -gt 0 -and $report.missing_durable_ids.Count -eq 0)
    $master = RedisMaster
    $hotKeys = Kube -Arguments @('exec', $master, '--', 'redis-cli', '--scan',
        '--pattern', "session:SIM-K6-$run-*")
    $report.remaining_hot_keys = @($hotKeys -split "`n" | Where-Object { $_.Trim() }).Count
    $report.archived_all_iterations = ($report.audit.rows -eq $report.lifecycle_iterations `
        -and $report.audit.invalid_transcripts -eq 0 -and $report.audit.nonterminal -eq 0 `
        -and $report.remaining_hot_keys -eq 0)
    $report.availability_thresholds_passed = ($report.k6_exit_code -eq 0)
    $report.recovery_criteria_passed = ($report.primary_change_observed `
        -and $report.durable_all_acknowledged -and $report.invalid_acknowledged_turn_ids.Count -eq 0 `
        -and $report.missing_acknowledged_archive_ids.Count -eq 0 -and $report.audit.nonterminal -eq 0)
    $report.transient_failures_allowed = $AllowTransientFailures.IsPresent
    if (-not $report.primary_change_observed) {
        throw $report.recovery_limitation
    }
    if (-not $report.durable_all_acknowledged -or $report.invalid_acknowledged_turn_ids.Count -gt 0) {
        throw 'Durable audit failed: missing acknowledged sessions or invalid acknowledged turns.'
    }
    if (-not $report.recovery_criteria_passed) {
        throw 'Recovery criteria failed; inspect archive and remaining-hot-state audit.'
    }
    if (-not $report.availability_thresholds_passed -and -not $AllowTransientFailures) {
        throw "k6 availability/latency thresholds failed (exit $($report.k6_exit_code)); inspect the saved summary."
    }
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    if ($null -ne $load -and -not $load.HasExited) {
        Stop-Process -Id $load.Id -Force
    }
    if ($null -ne $recording) { $report.k6_exit_code = Complete-RecordedProcess $recording }
    if ($null -ne $forward -and -not $forward.HasExited) {
        Stop-Process -Id $forward.Id -Force
        $forward.WaitForExit()
    }
    $report | ConvertTo-Json -Depth 10 | Set-Content "$output\$run-report.json"
    Write-Host "Evidence: $output\$run-report.json"
}
