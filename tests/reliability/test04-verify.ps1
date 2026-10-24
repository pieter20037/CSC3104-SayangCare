param([Parameter(Mandatory=$true)][string]$RunPrefix)
$ErrorActionPreference='Stop'
if($RunPrefix -notmatch '^[A-Za-z0-9_-]+$'){throw 'Invalid run prefix'}
Write-Host "Verifying prefix $RunPrefix (read-only)"
$sql = "SELECT id, state, version, jsonb_array_length(transcript->'turns') AS turn_count FROM sessions WHERE id LIKE '$RunPrefix%' ORDER BY id;"
docker compose exec -T postgres psql -U sayangcare -d sayangcare -c $sql
Write-Host 'Remaining hot Redis keys:'
docker compose exec -T redis-master redis-cli --scan --pattern "session:$RunPrefix*"
Write-Host 'Compare these rows against the run CSV. Missing Postgres rows MUST have a surviving Redis copy until recovery.'
