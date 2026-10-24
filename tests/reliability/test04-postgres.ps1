param([int]$Count=30, [string]$BaseUrl='http://localhost:8081/api/v1')
$ErrorActionPreference='Stop'
$run='TEST04-'+(Get-Date -Format 'yyyyMMddHHmmss')
$log="${run}-requests.csv"
Write-Host "Run ID: $run"
Write-Host 'Ensure Docker Desktop and API are running. This test STOPS PostgreSQL; run only in a disposable environment.'
$ok=Read-Host 'Type YES to stop PostgreSQL during the test'
if($ok -cne 'YES'){throw 'Cancelled'}
$ids=1..$Count | ForEach-Object {('{0}-{1:D3}' -f $run, $_)}
$results=@()
try {
  docker compose stop postgres | Out-Host
  foreach($id in $ids){
    $body=@{call_sid=$id;from='+6590000000';to='+6560000000'} | ConvertTo-Json
    $status='ok'; $errorText=''
    try {
      Invoke-RestMethod -Uri "$BaseUrl/calls/incoming" -Method Post -ContentType 'application/json' -Body $body -TimeoutSec 30 | Out-Null
      Invoke-RestMethod -Uri "$BaseUrl/calls/$id/hangup" -Method Post -TimeoutSec 30 | Out-Null
    } catch {$status='failed';$errorText=$_.Exception.Message}
    $results += [pscustomobject]@{session_id=$id;status=$status;error=$errorText;utc=[DateTime]::UtcNow.ToString('o')}
  }
} finally {
  docker compose start postgres | Out-Host
  $results | Export-Csv $log -NoTypeInformation
  Write-Host "Saved $log. Wait for sweeper recovery, then verify Postgres rows and Redis keys for prefix $run."
}
