[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [string]$KubectlPath = 'kubectl',
    [string]$Context = 'minikube',
    [string]$Namespace = 'sayangcare',
    [string]$OutputDirectory = 'artifacts\phase5-final\alerts'
)
$ErrorActionPreference = 'Stop'
if (-not $PSCmdlet.ShouldProcess("$Context/$Namespace", 'Create a synthetic escalation and replace one API pod to verify durable alerts')) {
    return
}
$prefix = @('--context', $Context, '-n', $Namespace)
$output = Join-Path (Resolve-Path "$PSScriptRoot\..\..").Path $OutputDirectory
New-Item -ItemType Directory -Force -Path $output | Out-Null
$sid = 'SIM-ALERT-' + [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')
$forwards = [System.Collections.Generic.List[object]]::new()
$report = [ordered]@{ session_id = $sid; started_utc = [DateTime]::UtcNow.ToString('o'); passed = $false; error = $null }
function Kube {
    param([string[]]$Arguments)
    $result = & $KubectlPath @prefix @Arguments
    if ($LASTEXITCODE -ne 0) { throw "kubectl failed: $($Arguments -join ' ')" }
    return ($result -join "`n")
}
function Api {
    param([int]$Port, [string]$Path, [string]$Body)
    $request = [System.Net.HttpWebRequest]::Create("http://127.0.0.1:$Port/api/v1/$Path")
    $request.Proxy = $null
    $request.Timeout = 30000
    if ($Body) {
        $request.Method = 'POST'
        $request.ContentType = 'application/json'
        $bytes = [System.Text.Encoding]::UTF8.GetBytes($Body)
        $request.ContentLength = $bytes.Length
        $stream = $request.GetRequestStream()
        try { $stream.Write($bytes, 0, $bytes.Length) } finally { $stream.Dispose() }
    }
    $response = $request.GetResponse()
    try {
        $reader = [System.IO.StreamReader]::new($response.GetResponseStream())
        try { return ($reader.ReadToEnd() | ConvertFrom-Json) } finally { $reader.Dispose() }
    } finally { $response.Dispose() }
}
function Forward {
    param([string]$Pod, [int]$Port)
    $process = Start-Process -FilePath $KubectlPath -ArgumentList @('--context', $Context,
        '-n', $Namespace, 'port-forward', "pod/$Pod", "${Port}:8080", '--address', '127.0.0.1') `
        -PassThru -NoNewWindow -RedirectStandardOutput "$output\$sid-$Pod-forward.log" `
        -RedirectStandardError "$output\$sid-$Pod-forward.err"
    $null = $process.Handle
    $forwards.Add($process)
    Start-Sleep -Seconds 2
    if ($process.HasExited) { throw "Forward for $Pod exited." }
    Api -Port $Port -Path 'ready' | Out-Null
    return $process
}
function AlertId {
    param([int]$Port)
    $alerts = Api -Port $Port -Path 'operator/alerts'
    $matching = @($alerts.alerts | Where-Object { $_.session_id -eq $sid -and $_.kind -eq 'escalated' })
    if ($matching.Count -ne 1) { throw "Expected one escalation alert for $sid on port $Port; got $($matching.Count)." }
    return $matching[0].id
}
function SqlCount {
    $cluster = (Kube -Arguments @('get', 'cluster', 'sayangcare-db', '-o', 'json')) | ConvertFrom-Json
    return [int](Kube -Arguments @('exec', $cluster.status.currentPrimary, '-c', 'postgres', '--',
        'psql', '-U', 'postgres', '-d', 'sayangcare', '-At', '-c',
        "SELECT count(*) FROM operator_alerts WHERE session_id='$sid' AND kind='escalated'"))
}
try {
    $pods = (Kube -Arguments @('get', 'pods', '-l', 'app=sayangcare-api', '-o', 'json')) | ConvertFrom-Json
    $ready = @($pods.items | Where-Object {
        -not $_.metadata.deletionTimestamp -and @($_.status.conditions | Where-Object {
            $_.type -eq 'Ready' -and $_.status -eq 'True'
        }).Count -eq 1
    })
    if ($ready.Count -lt 3) { throw 'Require at least three healthy API pods before replacing one.' }
    $a = $ready[0]; $b = $ready[1]
    $report.pod_a = $a.metadata.name; $report.pod_b = $b.metadata.name
    $report.uid_a = $a.metadata.uid; $report.uid_b = $b.metadata.uid
    $forwardA = Forward -Pod $a.metadata.name -Port 18187
    Forward -Pod $b.metadata.name -Port 18188 | Out-Null
    Api -Port 18187 -Path 'calls/incoming' -Body (@{
        call_sid = $sid; from = '+6590000000'; to = '+6560000000'
    } | ConvertTo-Json -Compress) | Out-Null
    $turn = Api -Port 18187 -Path "calls/$sid/turn" -Body '{"transcript":"I am thinking about suicide."}'
    if ($turn.risk_level -lt 4) { throw 'Synthetic high-risk turn did not escalate.' }
    $idA = AlertId -Port 18187; $idB = AlertId -Port 18188
    if ($idA -ne $idB) { throw 'Alert IDs differ before replacement.' }
    $report.alert_id = $idA
    $report.sql_count_before = SqlCount
    if ($report.sql_count_before -ne 1) { throw 'Expected exactly one durable escalation row.' }
    $report.deleted_utc = [DateTime]::UtcNow.ToString('o')
    Kube -Arguments @('delete', 'pod', $a.metadata.name, '--wait=false') | Out-Null
    if (-not $forwardA.HasExited) { Stop-Process -Id $forwardA.Id -Force; $forwardA.WaitForExit() }
    $deadline = [DateTime]::UtcNow.AddSeconds(120)
    $replacement = $null
    do {
        $pods = (Kube -Arguments @('get', 'pods', '-l', 'app=sayangcare-api', '-o', 'json')) | ConvertFrom-Json
        $replacement = @($pods.items | Where-Object {
            $_.metadata.uid -notin @($ready.metadata.uid) -and -not $_.metadata.deletionTimestamp `
                -and @($_.status.conditions | Where-Object {
                    $_.type -eq 'Ready' -and $_.status -eq 'True'
                }).Count -eq 1
        }) | Select-Object -First 1
        if ($null -ne $replacement) { break }
        Start-Sleep -Seconds 2
    } while ([DateTime]::UtcNow -lt $deadline)
    if ($null -eq $replacement) { throw 'No new Ready replacement pod observed.' }
    Forward -Pod $replacement.metadata.name -Port 18187 | Out-Null
    if ((AlertId -Port 18187) -ne $idA -or (AlertId -Port 18188) -ne $idA) {
        throw 'Expected alert changed or disappeared after replacement.'
    }
    $report.replacement_pod = $replacement.metadata.name
    $report.replacement_uid = $replacement.metadata.uid
    $report.sql_count_after = SqlCount
    if ($report.sql_count_after -ne 1) { throw 'Durable escalation row count changed.' }
    Api -Port 18188 -Path "calls/$sid/hangup" -Body '{}' | Out-Null
    $report.passed = $true
    $report.completed_utc = [DateTime]::UtcNow.ToString('o')
} catch {
    $report.error = $_.Exception.Message
    throw
} finally {
    foreach ($process in $forwards) {
        if (-not $process.HasExited) { Stop-Process -Id $process.Id -Force; $process.WaitForExit() }
    }
    $report | ConvertTo-Json -Depth 8 | Set-Content "$output\$sid.json"
    Write-Host "Evidence: $output\$sid.json"
}
