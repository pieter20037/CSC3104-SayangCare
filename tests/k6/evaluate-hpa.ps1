[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [string]$Context = 'minikube',
    [string]$Namespace = 'sayangcare',
    [string]$BaseUrl = 'http://127.0.0.1:18081',
    [int]$Vus = 100,
    [string]$Duration = '5m',
    [int]$PollSeconds = 5,
    [int]$DownscaleTimeoutSeconds = 900,
    [string]$K6Path,
    [string]$OutputDirectory = 'artifacts/perf/hpa'
)

$ErrorActionPreference = 'Stop'

if ($Vus -lt 1 -or $PollSeconds -lt 1 -or $DownscaleTimeoutSeconds -lt 0) {
    throw 'Vus and PollSeconds must be positive; DownscaleTimeoutSeconds cannot be negative.'
}
if (-not $PSCmdlet.ShouldProcess(
    "$Context/$Namespace",
    "Run a $Vus-VU k6 load test for $Duration and monitor HPA scaling"
)) {
    return
}

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..\')).Path
$outputPath = Join-Path $repositoryRoot $OutputDirectory
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$runId = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$summaryPath = Join-Path $outputPath "$runId-k6-summary.json"
$stdoutPath = Join-Path $outputPath "$runId-k6.stdout.log"
$stderrPath = Join-Path $outputPath "$runId-k6.stderr.log"
$samplesPath = Join-Path $outputPath "$runId-hpa-samples.csv"
$reportPath = Join-Path $outputPath "$runId-evaluation.json"
$runIdForCalls = "HPA-$runId"

if ($K6Path) {
    $k6Executable = $K6Path
} else {
    $k6Command = Get-Command k6 -ErrorAction Stop
    $k6Executable = $k6Command.Source
}

$script:kubectlPrefix = @('--context', $Context, '-n', $Namespace)
$script:samples = [System.Collections.Generic.List[object]]::new()

function Get-HpaSnapshot {
    param([string]$Phase)

    $hpaOutput = & kubectl @script:kubectlPrefix get hpa sayangcare-api -o json 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Could not read sayangcare-api HPA in context '$Context', namespace '$Namespace': $hpaOutput"
    }
    $hpa = ($hpaOutput | ConvertFrom-Json)

    $deploymentOutput = & kubectl @script:kubectlPrefix get deployment sayangcare-api -o json 2>&1
    if ($LASTEXITCODE -ne 0) {
        throw "Could not read sayangcare-api Deployment: $deploymentOutput"
    }
    $deployment = ($deploymentOutput | ConvertFrom-Json)

    $current = 0
    $desired = 0
    $ready = 0
    if ($null -ne $hpa.status.currentReplicas) { $current = [int]$hpa.status.currentReplicas }
    if ($null -ne $hpa.status.desiredReplicas) { $desired = [int]$hpa.status.desiredReplicas }
    if ($null -ne $deployment.status.readyReplicas) { $ready = [int]$deployment.status.readyReplicas }
    $currentMetrics = '[]'
    if ($null -ne $hpa.status.currentMetrics) {
        $currentMetrics = ConvertTo-Json -InputObject @($hpa.status.currentMetrics) -Compress -Depth 8
    }

    $sample = [pscustomobject]@{
        timestamp_utc = [DateTime]::UtcNow.ToString('o')
        phase = $Phase
        current_replicas = $current
        desired_replicas = $desired
        ready_replicas = $ready
        current_metrics_json = $currentMetrics
        hpa_conditions = (($hpa.status.conditions | ForEach-Object {
            '{0}:{1}:{2}' -f $_.type, $_.status, $_.reason
        }) -join ';')
    }
    $script:samples.Add($sample)
    return $sample
}

$initial = Get-HpaSnapshot -Phase 'before-load'
if ($initial.current_replicas -lt 1) {
    throw 'The HPA has no current replicas. Start the API Deployment before running the evaluation.'
}
if ($initial.desired_replicas -ne $initial.current_replicas `
    -or $initial.ready_replicas -lt $initial.current_replicas) {
    throw 'The HPA/Deployment is not stable at the starting replica count. Wait for the rollout and HPA to settle, then retry.'
}
$initialReplicas = $initial.current_replicas
$loadStarted = [DateTime]::UtcNow
$arguments = @(
    'run',
    '-e', "BASE_URL=$BaseUrl",
    '-e', "VUS=$Vus",
    '-e', "DURATION=$Duration",
    '-e', "RUN_ID=$runIdForCalls",
    '--summary-export', "`"$summaryPath`"",
    "`"$repositoryRoot\tests\k6\call-lifecycle.js`""
)
$process = Start-Process -FilePath $k6Executable -ArgumentList $arguments `
    -WorkingDirectory $repositoryRoot -NoNewWindow -PassThru `
    -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath

$firstDesiredScaleUp = $null
$firstAdditionalReady = $null
$maxDesiredReplicas = $initial.desired_replicas
$maxReadyReplicas = $initial.ready_replicas

try {
    while (-not $process.HasExited) {
        Start-Sleep -Seconds $PollSeconds
        $sample = Get-HpaSnapshot -Phase 'load'
        if ($sample.desired_replicas -gt $maxDesiredReplicas) {
            $maxDesiredReplicas = $sample.desired_replicas
        }
        if ($sample.ready_replicas -gt $maxReadyReplicas) {
            $maxReadyReplicas = $sample.ready_replicas
        }
        if ($null -eq $firstDesiredScaleUp -and $sample.desired_replicas -gt $initialReplicas) {
            $firstDesiredScaleUp = [DateTime]::Parse($sample.timestamp_utc).ToUniversalTime()
        }
        if ($null -eq $firstAdditionalReady -and $sample.ready_replicas -gt $initialReplicas) {
            $firstAdditionalReady = [DateTime]::Parse($sample.timestamp_utc).ToUniversalTime()
        }
    }
} catch {
    if (-not $process.HasExited) {
        Stop-Process -Id $process.Id -Force
        $process.WaitForExit()
    }
    throw
}
$process.Refresh()
$loadEnded = [DateTime]::UtcNow
$k6ExitCode = $process.ExitCode

$downscaleCompleted = $null
$downscaleDeadline = $loadEnded.AddSeconds($DownscaleTimeoutSeconds)
while ([DateTime]::UtcNow -lt $downscaleDeadline) {
    $sample = Get-HpaSnapshot -Phase 'cooldown'
    if ($sample.current_replicas -eq $initialReplicas `
        -and $sample.desired_replicas -eq $initialReplicas `
        -and $sample.ready_replicas -ge $initialReplicas) {
        $downscaleCompleted = [DateTime]::Parse($sample.timestamp_utc).ToUniversalTime()
        break
    }
    Start-Sleep -Seconds $PollSeconds
}

$script:samples | Export-Csv -Path $samplesPath -NoTypeInformation
$scaleUpDesiredSeconds = $null
$scaleUpReadySeconds = $null
$downscaleSeconds = $null
if ($null -ne $firstDesiredScaleUp) {
    $scaleUpDesiredSeconds = [math]::Round(($firstDesiredScaleUp - $loadStarted).TotalSeconds, 1)
}
if ($null -ne $firstAdditionalReady) {
    $scaleUpReadySeconds = [math]::Round(($firstAdditionalReady - $loadStarted).TotalSeconds, 1)
}
if ($null -ne $downscaleCompleted) {
    $downscaleSeconds = [math]::Round(($downscaleCompleted - $loadEnded).TotalSeconds, 1)
}

$report = [pscustomobject]@{
    run_id = $runIdForCalls
    context = $Context
    namespace = $Namespace
    vus = $Vus
    load_duration = $Duration
    load_started_utc = $loadStarted.ToString('o')
    load_ended_utc = $loadEnded.ToString('o')
    initial_replicas = $initialReplicas
    max_desired_replicas = $maxDesiredReplicas
    max_ready_replicas = $maxReadyReplicas
    first_scale_up_desired_seconds = $scaleUpDesiredSeconds
    first_additional_pod_ready_seconds = $scaleUpReadySeconds
    downscale_completed_utc = if ($null -ne $downscaleCompleted) { $downscaleCompleted.ToString('o') } else { $null }
    downscale_seconds_after_load = $downscaleSeconds
    downscale_observed = ($null -ne $downscaleCompleted)
    k6_exit_code = $k6ExitCode
    k6_summary_path = $summaryPath
    k6_stdout_path = $stdoutPath
    k6_stderr_path = $stderrPath
    hpa_samples_path = $samplesPath
}
$report | ConvertTo-Json -Depth 8 | Set-Content -Path $reportPath

Write-Host "HPA evaluation: $reportPath"
Write-Host "HPA samples: $samplesPath"
Write-Host "k6 summary: $summaryPath"
$report | Format-List

if ($k6ExitCode -ne 0) {
    exit $k6ExitCode
}
