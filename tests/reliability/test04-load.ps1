param(
  [int]$Count = 100,
  [int]$Throttle = 20,
  [string]$BaseUrl = 'http://localhost:8081/api/v1',
  [string]$Prefix = 'TEST04',
  [int]$TimeoutSec = 180
)
$ErrorActionPreference = 'Stop'
if ($PSVersionTable.PSVersion.Major -lt 7) { throw 'PowerShell 7 is required. Run with pwsh.' }
if ($Count -lt 1 -or $Throttle -lt 1) { throw 'Count and Throttle must be positive.' }
$runId = '{0}-{1}' -f $Prefix, ([DateTime]::UtcNow.ToString('yyyyMMddHHmmssfff'))
Write-Host "Run prefix: $runId"
$started = [DateTime]::UtcNow
$results = 1..$Count | ForEach-Object -Parallel {
  $sid = '{0}-{1:D4}' -f $using:runId, $_
  $url = $using:BaseUrl
  $timeout = $using:TimeoutSec
  $body = @{call_sid=$sid;from='+6590000000';to='+6560000000'} | ConvertTo-Json -Compress
  $sw = [System.Diagnostics.Stopwatch]::StartNew()
  $phase = 'incoming'; $status = 'ok'; $errorText = ''
  try {
    Invoke-RestMethod -Uri "$url/calls/incoming" -Method Post -ContentType 'application/json' -Body $body -TimeoutSec $timeout | Out-Null
    $phase = 'hangup'
    Invoke-RestMethod -Uri "$url/calls/$sid/hangup" -Method Post -TimeoutSec $timeout | Out-Null
  } catch { $status = 'failed'; $errorText = $_.Exception.Message }
  $sw.Stop()
  [pscustomobject]@{session_id=$sid;status=$status;phase=$phase;duration_ms=[math]::Round($sw.Elapsed.TotalMilliseconds,1);error=$errorText;utc=[DateTime]::UtcNow.ToString('o')}
} -ThrottleLimit $Throttle
$ended = [DateTime]::UtcNow
$outFile = "${runId}-requests.csv"
$results | Export-Csv -Path $outFile -NoTypeInformation
$ok = @($results | Where-Object status -eq 'ok').Count
$failed = @($results | Where-Object status -ne 'ok').Count
$durations = @($results | ForEach-Object duration_ms | Sort-Object)
function Percentile([object[]]$Values, [double]$P) { if ($Values.Count -eq 0) {return 0}; $index=[math]::Ceiling($P*$Values.Count)-1; return $Values[[math]::Max(0,$index)] }
$summary = [pscustomobject]@{
  run_prefix=$runId;started_utc=$started.ToString('o');ended_utc=$ended.ToString('o');count=$Count;throttle=$Throttle;
  success=$ok;failed=$failed;duration_seconds=[math]::Round(($ended-$started).TotalSeconds,2);
  throughput_sessions_per_second=[math]::Round($Count/[math]::Max(0.001,($ended-$started).TotalSeconds),2);
  p95_lifecycle_ms=(Percentile $durations 0.95);p99_lifecycle_ms=(Percentile $durations 0.99)
}
$summary | ConvertTo-Json | Set-Content "${runId}-summary.json"
$summary | Format-List
Write-Host "CSV: $outFile"
Write-Host "IMPORTANT: These are full-lifecycle durations, NOT per-HTTP-request p95/p99. Verify archived sessions separately."
