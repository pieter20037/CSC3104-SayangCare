param(
  [string]$PodA='http://localhost:8081',
  [string]$PodB='http://localhost:8082',
  [Parameter(Mandatory=$true)][string]$SessionId
)
$ErrorActionPreference='Stop'
if ($PodA.TrimEnd('/') -eq $PodB.TrimEnd('/')) { throw 'Use different pod endpoints.' }
Write-Host 'Use two different API pods/port-forwards backed by the same PostgreSQL database.'
$a=Invoke-RestMethod "$PodA/api/v1/operator/alerts"
$b=Invoke-RestMethod "$PodB/api/v1/operator/alerts"
if (@($a.alerts | Where-Object session_id -eq $SessionId).Count -eq 0 `
    -or @($b.alerts | Where-Object session_id -eq $SessionId).Count -eq 0) {
  throw "FAIL: expected alert for session $SessionId is missing from at least one replica."
}
$aid=@($a.alerts | ForEach-Object id | Sort-Object)
$bid=@($b.alerts | ForEach-Object id | Sort-Object)
if(($aid -join ',') -ne ($bid -join ',')){throw 'FAIL: pod alert IDs differ'}
Write-Host "PASS: both replicas returned the same $($aid.Count) alert IDs (latest 25)."
Write-Host 'Next create an alert through PodA, compare again, replace PodA, and compare again.'
