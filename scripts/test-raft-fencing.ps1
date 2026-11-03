#requires -Version 5.1
<#
.SYNOPSIS
Exercises ownership fencing over HTTP and optionally stops verified Compose Raft members.
.DESCRIPTION
Uses only unique SIM sessions and one fresh crisis turn per session. The built-in
keyword takes the first-turn immediate-escalation branch in call.rs, bypassing
inference; never substitute a low-risk turn or replay that turn on an old session.
Default execution performs correctness and latency tests, not container faults.

Fault injection requires -InjectFaults and ShouldProcess approval (or an explicit
-Confirm:$false). Only previously running, verified members stopped by this run
are restarted. No volumes, environment files, or container environments are read.

Optional SQL audit uses an explicitly supplied running PostgreSQL container and
local psql trust/peer authentication. No credentials are accepted or printed.
Without that option SQL evidence is explicitly NOT RUN, not a passing SQL audit.
.EXAMPLE
.\scripts\test-raft-fencing.ps1 -SkipPostgres -SampleCount 5
.EXAMPLE
.\scripts\test-raft-fencing.ps1 -LatencyOnly -WarmupCount 2 -SampleCount 20
.EXAMPLE
.\scripts\test-raft-fencing.ps1 -InjectFaults -ComposeProject sayangcare -Confirm:$false
#>
[CmdletBinding(SupportsShouldProcess = $true, ConfirmImpact = 'High')]
param(
    [string[]]$RaftUrls = @('http://localhost:18081', 'http://localhost:18082', 'http://localhost:18083'),
    [string]$PostgresUrl = 'http://localhost:18080',
    [switch]$SkipPostgres,
    [switch]$LatencyOnly,
    [switch]$InjectFaults,
    [ValidateRange(1, 10000)][int]$SampleCount = 10,
    [ValidateRange(0, 1000)][int]$WarmupCount = 2,
    [ValidateRange(1, 120)][int]$HttpTimeoutSeconds = 12,
    [ValidateRange(1, 120)][int]$UnavailableBoundSeconds = 10,
    [ValidateRange(5, 600)][int]$ElectionTimeoutSeconds = 45,
    [ValidateRange(5, 600)][int]$RecoveryTimeoutSeconds = 90,
    [ValidateRange(1, 60)][int]$StopTimeoutSeconds = 5,
    [string]$ComposeProject,
    [string]$AuditPostgresContainer,
    [string]$AuditDatabase = 'sayangcare',
    [string]$AuditUser = 'sayangcare',
    [string]$RaftDatabaseTopology = 'Not independently verified; typically Docker single PostgreSQL',
    [string]$PostgresDatabaseTopology = 'Not independently verified; port 18080 may be Minikube/replicated PostgreSQL',
    [switch]$LibraryOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Initialize-FencingTransport {
    Add-Type -AssemblyName System.Net.Http
    if ('FencingHarness.Transport' -as [type]) { return }
    Add-Type -ReferencedAssemblies System.Net.Http -TypeDefinition @'
using System;
using System.Diagnostics;
using System.Net;
using System.Net.Http;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
namespace FencingHarness {
    public sealed class Reply {
        public int Status;
        public double Milliseconds;
        public string Body;
        public string Error;
    }
    public static class Transport {
        private static readonly HttpClient Client = Create();
        private static HttpClient Create() {
            ServicePointManager.DefaultConnectionLimit = 32;
            return new HttpClient(new HttpClientHandler {
                UseProxy = false, AllowAutoRedirect = false, UseCookies = false
            }) { Timeout = Timeout.InfiniteTimeSpan };
        }
        private static async Task<Reply> Send(string url, string method, string body,
                                             int timeoutMs, Task gate) {
            await gate.ConfigureAwait(false);
            var watch = Stopwatch.StartNew();
            var result = new Reply();
            using (var cancellation = new CancellationTokenSource(timeoutMs))
            using (var request = new HttpRequestMessage(new HttpMethod(method), url)) {
                if (!String.IsNullOrEmpty(body))
                    request.Content = new StringContent(body, Encoding.UTF8, "application/json");
                try {
                    using (var response = await Client.SendAsync(request,
                        HttpCompletionOption.ResponseContentRead, cancellation.Token).ConfigureAwait(false)) {
                        result.Status = (int)response.StatusCode;
                        result.Body = await response.Content.ReadAsStringAsync().ConfigureAwait(false);
                    }
                } catch (Exception e) {
                    result.Error = e.GetType().Name + ": " + e.Message;
                }
            }
            result.Milliseconds = watch.Elapsed.TotalMilliseconds;
            return result;
        }
        public static Reply Request(string url, string method, string body, int timeoutMs) {
            return Send(url, method, body, timeoutMs, Task.FromResult(0)).GetAwaiter().GetResult();
        }
        public static Reply[] Race(string[] urls, string[] bodies, int timeoutMs) {
            // Both async requests are registered before a single shared start barrier opens.
            var gate = new TaskCompletionSource<int>(TaskCreationOptions.RunContinuationsAsynchronously);
            var requests = new Task<Reply>[urls.Length];
            for (int i = 0; i < urls.Length; i++)
                requests[i] = Send(urls[i], "POST", bodies[i], timeoutMs, gate.Task);
            gate.SetResult(0);
            return Task.WhenAll(requests).GetAwaiter().GetResult();
        }
    }
}
'@
}

function Get-Field($Object, [string]$Name) {
    if ($null -eq $Object) { return $null }
    if ($Object -is [System.Collections.IDictionary]) { return $Object[$Name] }
    $property = $Object.PSObject.Properties[$Name]
    if ($null -ne $property) { return $property.Value }
    return $null
}

function ConvertTo-ApiUrl([string]$BaseUrl) {
    $uri = $null
    if (-not [Uri]::TryCreate($BaseUrl, [UriKind]::Absolute, [ref]$uri) -or
        $uri.Scheme -notin @('http', 'https') -or $uri.UserInfo -or $uri.Query -or $uri.Fragment) {
        throw 'URLs must be absolute HTTP(S) URLs without credentials, query, or fragment.'
    }
    $path = $uri.AbsolutePath.TrimEnd('/')
    if ($path -notin @('', '/api/v1')) { throw 'URL path must be empty or /api/v1.' }
    return $uri.GetLeftPart([UriPartial]::Authority).TrimEnd('/') + '/api/v1'
}

function New-OperationId { return 'fencing-' + [Guid]::NewGuid().ToString('N') }

function Save-Evidence {
    if ($null -ne $script:Report) {
        $script:Report.updated_utc = [DateTime]::UtcNow.ToString('o')
        $script:Report | ConvertTo-Json -Depth 40 |
            Set-Content -LiteralPath (Join-Path $script:EvidenceDirectory 'report.json') -Encoding UTF8
    }
}

function Assert-Fencing([bool]$Condition, [string]$Name, $Observed) {
    $entry = [ordered]@{
        name = $Name; passed = $Condition; observed = $Observed
        utc = [DateTime]::UtcNow.ToString('o')
    }
    $script:Report.assertions.Add($entry)
    if (-not $Condition) {
        Save-Evidence
        throw "Assertion failed: $Name"
    }
}

function Record-Reply($Raw, [string]$Url, [string]$Operation, [string]$Phase, $Body) {
    $json = $null
    $parseError = $null
    if ($Raw.Body) {
        try { $json = $Raw.Body | ConvertFrom-Json } catch { $parseError = 'Response was not JSON' }
    }
    $entry = [pscustomobject][ordered]@{
        utc = [DateTime]::UtcNow.ToString('o'); url = $Url; operation = $Operation
        phase = $Phase; request = $Body; status = $Raw.Status
        milliseconds = $Raw.Milliseconds; error = $Raw.Error
        parse_error = $parseError; response = $json; raw_body = $Raw.Body
    }
    $script:Report.requests.Add($entry)
    $entry | ConvertTo-Json -Depth 30 -Compress |
        Add-Content -LiteralPath (Join-Path $script:EvidenceDirectory 'requests.jsonl') -Encoding UTF8
    return $entry
}

function Send-Fencing([string]$Base, [string]$Path, [string]$Method = 'GET', $Body = $null,
    [string]$Operation = 'diagnostic', [string]$Phase = 'correctness', [int]$TimeoutMs = 0) {
    if ($TimeoutMs -le 0) { $TimeoutMs = $HttpTimeoutSeconds * 1000 }
    $payload = $null
    if ($null -ne $Body) { $payload = ConvertTo-Json -InputObject $Body -Depth 20 -Compress }
    $url = $Base + $Path
    $reply = [FencingHarness.Transport]::Request($url, $Method, $payload, $TimeoutMs)
    return Record-Reply $reply $url $Operation $Phase $Body
}

function Expect-Status($Reply, [int]$Status, [string]$Name) {
    Assert-Fencing ($Reply.status -eq $Status) "$Name expected HTTP $Status" @{
        status = $Reply.status; milliseconds = $Reply.milliseconds; error = $Reply.error
    }
}

function Get-Ownership([string]$Base, [string]$Session) {
    $reply = Send-Fencing $Base "/calls/$Session/ownership"
    Expect-Status $reply 200 "ownership $Session"
    foreach ($field in @('volunteer_id', 'assignment_generation', 'pending_operation_id', 'coordinator')) {
        Assert-Fencing ($null -ne $reply.response -and
            $reply.response.PSObject.Properties.Name -contains $field) "ownership includes $field" $reply.response
    }
    Assert-Fencing ($null -ne (Get-Field $reply.response 'assignment_generation')) 'ownership generation is not null' $reply.response
    return $reply.response
}

function Get-CaseAudit([string]$Base, [string]$Session) {
    $reply = Send-Fencing $Base "/calls/$Session/ownership/audit" GET $null 'api-sql-audit'
    Expect-Status $reply 200 "API-exposed SQL audit $Session"
    foreach ($field in @('session_id', 'history', 'notes')) {
        Assert-Fencing ($null -ne $reply.response -and
            $reply.response.PSObject.Properties.Name -contains $field) "API-exposed SQL audit includes $field" $reply.response
    }
    Assert-Fencing ($reply.response.session_id -eq $Session) 'SQL audit corresponds to requested case' $reply.response
    $script:Report.api_exposed_sql_audits.Add(@{
        source = 'API-exposed PostgreSQL queries; not independently executed admin SQL'
        url = $reply.url; utc = $reply.utc; result = $reply.response
    })
    return $reply.response
}

function Assert-AuditOwner($Audit, [string]$Volunteer, [long]$Generation, [string]$Name) {
    $active = @($Audit.history | Where-Object { $_.active -eq $true })
    Assert-Fencing ($active.Count -eq 1 -and $active[0].volunteer_id -eq $Volunteer -and
        $active[0].assignment_generation -eq $Generation) $Name $Audit
}

function Assert-Ownership($Ownership, $Volunteer, [long]$Generation, [string]$Name) {
    Assert-Fencing ((Get-Field $Ownership 'volunteer_id') -eq $Volunteer -and
        (Get-Field $Ownership 'assignment_generation') -eq $Generation -and
        $null -eq (Get-Field $Ownership 'pending_operation_id')) $Name $Ownership
}

function New-SyntheticCase([string]$Base, [string]$Mode, [string]$Phase = 'correctness') {
    $sid = 'SIM-fencing-' + [Guid]::NewGuid().ToString('N')
    $script:Report.sessions.Add([ordered]@{ session_id = $sid; coordinator = $Mode; base_url = $Base })
    $created = Send-Fencing $Base '/calls/incoming' POST @{
        call_sid = $sid; from = 'SIM-FENCING-LOCAL'; to = 'SIM-FENCING-NO-DIAL'
    } 'create' $Phase
    Expect-Status $created 200 'create unique synthetic case'
    Assert-Fencing ((Get-Field $created.response 'session_id') -eq $sid) 'incoming returns requested unique id' $created.response
    # A fresh first turn with this risk.rs keyword bypasses real inference.
    $turn = Send-Fencing $Base "/calls/$sid/turn" POST @{
        transcript = 'Synthetic fencing test fixture: suicide'
    } 'escalate' $Phase
    Expect-Status $turn 200 'first-turn deterministic risk escalation'
    return $sid
}

function Claim-Case([string]$Base, [string]$Session, [string]$Volunteer,
    [string]$Phase = 'correctness', [string]$RequestId = '', [int]$TimeoutMs = 0, [switch]$Reclaim) {
    if (-not $RequestId) { $RequestId = New-OperationId }
    $operation = 'claim'
    if ($Reclaim) { $operation = 'reclaim' }
    return Send-Fencing $Base '/volunteers/claim' POST @{
        session_id = $Session; volunteer_id = $Volunteer; request_id = $RequestId; simulate = $true
    } $operation $Phase $TimeoutMs
}

function Act-Case([string]$Base, [string]$Session, [string]$Volunteer,
    [long]$Generation, [string]$Action, [string]$Target = '',
    [string]$Phase = 'correctness', [string]$RequestId = '', [int]$TimeoutMs = 0) {
    if (-not $RequestId) { $RequestId = New-OperationId }
    $body = @{ assignment_generation = $Generation; request_id = $RequestId }
    if ($Target) { $body.to_volunteer_id = $Target }
    if ($Action -eq 'notes') { $body.body = 'Synthetic fencing evidence; no patient information.' }
    return Send-Fencing $Base "/volunteers/$Volunteer/cases/$Session/$Action" POST $body $Action $Phase $TimeoutMs
}

function Assert-Transition($Reply, [string]$Mode, [string]$Session, $Volunteer,
    [long]$Generation, [string]$Name) {
    Expect-Status $Reply 200 $Name
    $value = $Reply.response
    foreach ($field in @('volunteer_id', 'assignment_generation', 'session_id', 'request_id', 'coordinator')) {
        Assert-Fencing ($null -ne $value -and $value.PSObject.Properties.Name -contains $field) "$Name response includes $field" $value
    }
    Assert-Fencing ((Get-Field $value 'coordinator') -eq $Mode -and
        (Get-Field $value 'session_id') -eq $Session -and
        (Get-Field $value 'volunteer_id') -eq $Volunteer -and
        (Get-Field $value 'assignment_generation') -eq $Generation -and
        (Get-Field $value 'request_id') -eq $Reply.request.request_id) "$Name ownership and request identity" $value
}

function Get-RaftStatus([string]$Base) {
    $reply = Send-Fencing $Base '/queue/raft'
    Expect-Status $reply 200 'Raft diagnostic'
    Assert-Fencing ((Get-Field $reply.response 'backend') -eq 'raft') 'diagnostic backend really Raft' $reply.response
    foreach ($field in @('node_id', 'current_leader', 'current_term', 'last_applied', 'ownership')) {
        Assert-Fencing ($reply.response.PSObject.Properties.Name -contains $field) "Raft diagnostic includes $field" $reply.response
    }
    return $reply.response
}

function Test-Preflight([string[]]$Urls, [string]$Mode) {
    foreach ($base in $Urls) {
        foreach ($endpoint in @('/health', '/ready')) {
            $reply = Send-Fencing $base $endpoint
            Expect-Status $reply 200 "$Mode storage/readiness preflight $base$endpoint"
        }
        # A never-created synthetic ownership read must expose the actual coordinator.
        $probe = 'SIM-fencing-preflight-' + [Guid]::NewGuid().ToString('N')
        $ownership = Get-Ownership $base $probe
        Assert-Fencing ((Get-Field $ownership 'coordinator') -eq $Mode) "$base actual ownership coordinator is $Mode" $ownership
        $status = Send-Fencing $base '/queue/raft'
        Expect-Status $status 200 'preflight backend'
        $expected = 'redis'
        if ($Mode -eq 'raft') { $expected = 'raft' }
        Assert-Fencing ((Get-Field $status.response 'backend') -eq $expected) "$base backend is $expected" $status.response
        $script:Report.preflight.Add([ordered]@{ url = $base; coordinator = $ownership.coordinator; diagnostic = $status.response })
    }
    if ($Mode -eq 'raft') {
        $statuses = @($Urls | ForEach-Object { Get-RaftStatus $_ })
        $ids = @($statuses | ForEach-Object { $_.node_id } | Sort-Object -Unique)
        $leaders = @($statuses | ForEach-Object { $_.current_leader } | Sort-Object -Unique)
        Assert-Fencing ($ids.Count -eq 3) 'three distinct Raft members' $ids
        Assert-Fencing ($leaders.Count -eq 1 -and $null -ne $leaders[0] -and $leaders[0] -in $ids) 'all members agree on a present leader' $leaders
    }
}

function Test-Correctness([string[]]$Urls, [string]$Mode) {
    $base = $Urls[0]
    $second = $Urls[$Urls.Count - 1]
    $sid = New-SyntheticCase $base $Mode
    $bodies = @(
        @{ session_id = $sid; volunteer_id = 'volunteer-aisha'; request_id = (New-OperationId); simulate = $true },
        @{ session_id = $sid; volunteer_id = 'volunteer-noor'; request_id = (New-OperationId); simulate = $true }
    )
    $raceUrls = [string[]]@("$base/volunteers/claim", "$second/volunteers/claim")
    $encoded = [string[]]@($bodies | ForEach-Object { ConvertTo-Json -InputObject $_ -Compress })
    $raw = [FencingHarness.Transport]::Race($raceUrls, $encoded, $HttpTimeoutSeconds * 1000)
    $results = @(
        (Record-Reply $raw[0] $raceUrls[0] 'claim' 'race' $bodies[0]),
        (Record-Reply $raw[1] $raceUrls[1] 'claim' 'race' $bodies[1])
    )
    $codes = @($results | ForEach-Object { $_.status } | Sort-Object)
    Assert-Fencing (($codes -join ',') -eq '200,409') "$Mode FENCE-01 exactly one winner" $codes
    $winner = @($results | Where-Object { $_.status -eq 200 })[0]
    $owner = $winner.request.volunteer_id
    $loser = 'volunteer-aisha'
    if ($owner -eq 'volunteer-aisha') { $loser = 'volunteer-noor' }
    Assert-Transition $winner $Mode $sid $owner 1 "$Mode race winner"
    Assert-Ownership (Get-Ownership $base $sid) $owner 1 "$Mode single durable owner after race"
    Assert-AuditOwner (Get-CaseAudit $base $sid) $owner 1 "$Mode FENCE-01 API-exposed SQL has exactly one active assignment"
    $transferred = Act-Case $second $sid $owner 1 'transfer' $loser
    Assert-Transition $transferred $Mode $sid $loser 2 "$Mode transfer race winner to loser"
    $released = Act-Case $base $sid $loser 2 'resolve'
    Assert-Transition $released $Mode $sid $null 3 "$Mode race case cleanup"

    $sid = New-SyntheticCase $base $Mode
    $claim = Claim-Case $base $sid 'volunteer-aisha'
    Assert-Transition $claim $Mode $sid 'volunteer-aisha' 1 "$Mode claim A"
    $replay = Claim-Case $base $sid 'volunteer-aisha' 'correctness' $claim.request.request_id
    Assert-Transition $replay $Mode $sid 'volunteer-aisha' 1 "$Mode idempotent claim replay"
    $conflict = Claim-Case $base $sid 'volunteer-noor' 'correctness' $claim.request.request_id
    Expect-Status $conflict 409 "$Mode conflicting request id reuse"
    $transfer = Act-Case $second $sid 'volunteer-aisha' 1 'transfer' 'volunteer-noor'
    Assert-Transition $transfer $Mode $sid 'volunteer-noor' 2 "$Mode transfer A to B"
    $staleNoteIds = @()
    foreach ($action in @('notes', 'transfer', 'release')) {
        $target = ''
        if ($action -eq 'transfer') { $target = 'volunteer-noor' }
        $stale = Act-Case $base $sid 'volunteer-aisha' 1 $action $target
        if ($action -eq 'notes') { $staleNoteIds += $stale.request.request_id }
        Expect-Status $stale 409 "$Mode FENCE-02 stale $action rejected"
        Assert-Ownership (Get-Ownership $second $sid) 'volunteer-noor' 2 "$Mode stale $action leaves ownership unchanged"
    }
    $note = Act-Case $second $sid 'volunteer-noor' 2 'notes'
    Expect-Status $note 200 "$Mode current owner note accepted"
    $audit = Get-CaseAudit $second $sid
    Assert-AuditOwner $audit 'volunteer-noor' 2 "$Mode FENCE-02 stale actions leave one active SQL owner"
    Assert-Fencing (@($audit.notes | Where-Object { $_.request_id -in $staleNoteIds }).Count -eq 0) "$Mode rejected notes absent from API-exposed SQL audit" $audit
    $accepted = @($audit.notes | Where-Object { $_.request_id -eq $note.request.request_id })
    Assert-Fencing ($accepted.Count -eq 1 -and $accepted[0].volunteer_id -eq 'volunteer-noor' -and
        $accepted[0].assignment_generation -eq 2) "$Mode current note persisted exactly once with correct SQL token" $audit
    $back = Act-Case $base $sid 'volunteer-noor' 2 'transfer' 'volunteer-aisha'
    Assert-Transition $back $Mode $sid 'volunteer-aisha' 3 "$Mode ABA return to A"
    Expect-Status (Act-Case $base $sid 'volunteer-aisha' 1 'notes') 409 "$Mode ABA old token still rejected"
    $release = Act-Case $base $sid 'volunteer-aisha' 3 'release'
    Assert-Transition $release $Mode $sid $null 4 "$Mode release increments"
    Assert-Ownership (Get-Ownership $base $sid) $null 4 "$Mode release tombstone"
    $reclaim = Claim-Case $base $sid 'volunteer-noor'
    Assert-Transition $reclaim $Mode $sid 'volunteer-noor' 5 "$Mode reclaim preserves counter"
    $resolved = Act-Case $base $sid 'volunteer-noor' 5 'resolve'
    Assert-Transition $resolved $Mode $sid $null 6 "$Mode resolve revokes ownership"
    Assert-Ownership (Get-Ownership $base $sid) $null 6 "$Mode final correctness state"
}

function Get-LatencyStatistics([object[]]$Requests) {
    $groups = @($Requests | Where-Object { $_.phase -eq 'sample' } | Group-Object operation)
    foreach ($group in $groups) {
        $successes = @($group.Group | Where-Object { $_.status -eq 200 } | Sort-Object milliseconds)
        $median = $null
        $p95 = $null
        if ($successes.Count -gt 0) {
            $middle = [int][Math]::Floor($successes.Count / 2)
            $median = $successes[$middle].milliseconds
            if ($successes.Count % 2 -eq 0) {
                $median = ($successes[$middle - 1].milliseconds + $successes[$middle].milliseconds) / 2
            }
            $p95 = $successes[[int][Math]::Ceiling(0.95 * $successes.Count) - 1].milliseconds
        }
        [pscustomobject]@{
            operation = $group.Name; attempted = $group.Count
            successful = $successes.Count; failures = $group.Count - $successes.Count
            timeouts_or_transport_errors = @($group.Group | Where-Object { $_.status -eq 0 }).Count
            median_ms = $median; p95_ms = $p95
        }
    }
}

function Test-Latency([string]$Base, [string]$Mode) {
    $start = $script:Report.requests.Count
    for ($i = 0; $i -lt ($WarmupCount + $SampleCount); $i++) {
        $phase = 'sample'
        if ($i -lt $WarmupCount) { $phase = 'warmup' }
        $sid = New-SyntheticCase $Base $Mode $phase
        Assert-Transition (Claim-Case $Base $sid 'volunteer-aisha' $phase) $Mode $sid 'volunteer-aisha' 1 "$Mode latency claim"
        Assert-Transition (Act-Case $Base $sid 'volunteer-aisha' 1 'transfer' 'volunteer-noor' $phase) $Mode $sid 'volunteer-noor' 2 "$Mode latency transfer"
        Expect-Status (Act-Case $Base $sid 'volunteer-noor' 2 'notes' '' $phase) 200 "$Mode latency note"
        Assert-Transition (Act-Case $Base $sid 'volunteer-noor' 2 'release' '' $phase) $Mode $sid $null 3 "$Mode latency release"
        Assert-Transition (Claim-Case $Base $sid 'volunteer-aisha' $phase -Reclaim) $Mode $sid 'volunteer-aisha' 4 "$Mode latency reclaim"
        Assert-Transition (Act-Case $Base $sid 'volunteer-aisha' 4 'resolve' '' $phase) $Mode $sid $null 5 "$Mode latency resolve"
    }
    $requests = @($script:Report.requests | Select-Object -Skip $start)
    $script:Report.latency[$Mode] = @(Get-LatencyStatistics $requests)
}

function Invoke-Docker([string[]]$Arguments, [string]$InputText = '', [int]$TimeoutSeconds = 30) {
    foreach ($argument in $Arguments) {
        if ($argument.Contains("`n") -or $argument.Contains("`r")) {
            throw 'Unsafe Docker argument.'
        }
    }
    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = 'docker'
    $info.Arguments = ($Arguments | ForEach-Object {
        $escaped = [regex]::Replace($_, '(\\*)"', '$1$1\"')
        $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
        '"' + $escaped + '"'
    }) -join ' '
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $info.RedirectStandardInput = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $info
    try {
        if (-not $process.Start()) { throw 'Could not start Docker CLI.' }
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if ($InputText) { $process.StandardInput.Write($InputText) }
        $process.StandardInput.Close()
        if (-not $process.WaitForExit($TimeoutSeconds * 1000)) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            throw "Docker command exceeded ${TimeoutSeconds}s."
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errorText = $stderr.GetAwaiter().GetResult()
        $script:Report.native_commands.Add([ordered]@{
            executable = 'docker'; arguments = $Arguments; exit_code = $process.ExitCode
            utc = [DateTime]::UtcNow.ToString('o')
        })
        if ($process.ExitCode -ne 0) { throw "Docker command failed ($($process.ExitCode)): $errorText" }
        return $output.Trim()
    } finally { $process.Dispose() }
}

function Get-ComposeMembers {
    $root = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $raftFile = Join-Path $root 'docker-compose.raft.yml'
    $projects = Invoke-Docker @('compose', 'ls', '--format', 'json') | ConvertFrom-Json
    $matches = @($projects | Where-Object {
        $files = @($_.ConfigFiles -split ',')
        @($files | Where-Object { [IO.Path]::GetFullPath($_) -eq $raftFile }).Count -gt 0
    })
    if ($ComposeProject) { $matches = @($matches | Where-Object { $_.Name -eq $ComposeProject }) }
    Assert-Fencing ($matches.Count -eq 1) 'exactly one Compose project for this checkout/raft file' @($matches | ForEach-Object { $_.Name })
    $project = $matches[0].Name
    # Supply a secret-free stdin model and NUL env file: ps must not load .env.
    $model = '{"services":{"raft-node-1":{"image":"unused"},"raft-node-2":{"image":"unused"},"raft-node-3":{"image":"unused"}}}'
    $members = @()
    foreach ($number in 1..3) {
        $service = "raft-node-$number"
        $idsText = Invoke-Docker @('compose', '--env-file', 'NUL', '--project-name', $project,
            '--file', '-', 'ps', '--all', '--quiet', $service) $model
        $ids = @($idsText -split '\r?\n' | Where-Object { $_ })
        Assert-Fencing ($ids.Count -eq 1 -and $ids[0] -match '^[a-f0-9]{12,64}$') "one exact container for $project/$service" $ids
        $id = $ids[0]
        $template = '{{index .Config.Labels "com.docker.compose.project"}}|{{index .Config.Labels "com.docker.compose.service"}}|{{.State.Running}}|{{.Config.Image}}|{{.Image}}'
        $details = Invoke-Docker @('inspect', '--format', $template, $id)
        $parts = $details -split '\|'
        Assert-Fencing ($parts.Count -eq 5 -and $parts[0] -eq $project -and $parts[1] -eq $service -and $parts[2] -eq 'true') "labels and running state $service" $details
        $diagnostic = Get-RaftStatus $script:RaftBases[$number - 1]
        Assert-Fencing ($diagnostic.node_id -eq $number) "$service URL has matching Raft node id" $diagnostic
        $uri = [Uri]$script:RaftBases[$number - 1]
        Assert-Fencing ($uri.Host -in @('localhost', '127.0.0.1', '::1')) 'fault URLs must target local published ports' $uri.Host
        $ports = Invoke-Docker @('port', $id, '8080/tcp')
        Assert-Fencing (@($ports -split '\r?\n' | Where-Object { $_ -match (':' + $uri.Port + '$') }).Count -gt 0) "$service URL matches published port" $ports
        $members += [pscustomobject]@{ id = $id; service = $service; node_id = $number
            url = $script:RaftBases[$number - 1]; project = $project; image = $parts[3]; image_id = $parts[4] }
    }
    $script:Report.containers = $members
    return $members
}

function Stop-OwnedMember($Member) {
    # Register before stop: a timed-out CLI may still have stopped this container.
    $script:Stopped[$Member.id] = $Member
    Save-Evidence
    $null = Invoke-Docker @('stop', '--time', "$StopTimeoutSeconds", $Member.id) '' ($StopTimeoutSeconds + 15)
    $running = Invoke-Docker @('inspect', '--format', '{{.State.Running}}', $Member.id)
    Assert-Fencing ($running -eq 'false') "stopped exact member $($Member.service)" $running
}

function Wait-Readiness($Members) {
    $deadline = [DateTime]::UtcNow.AddSeconds($RecoveryTimeoutSeconds)
    do {
        $allReady = $true
        foreach ($member in $Members) {
            $remaining = [int][Math]::Max(1, ($deadline - [DateTime]::UtcNow).TotalMilliseconds)
            $ready = Send-Fencing $member.url '/ready' GET $null 'readiness' 'recovery' ([Math]::Min(2000, $remaining))
            if ($ready.status -ne 200) { $allReady = $false }
        }
        if ($allReady -and [DateTime]::UtcNow -le $deadline) { return }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Readiness restoration exceeded ${RecoveryTimeoutSeconds}s."
}

function Restore-OwnedMembers {
    $errors = New-Object 'System.Collections.Generic.List[string]'
    $members = @($script:Stopped.Values)
    foreach ($member in $members) {
        try {
            $null = Invoke-Docker @('start', $member.id)
        } catch { $errors.Add("$($member.service): $($_.Exception.Message)") }
    }
    if ($members.Count -gt 0) {
        try { Wait-Readiness $members } catch { $errors.Add($_.Exception.Message) }
    }
    if ($errors.Count -gt 0) { throw ('Restoration failed: ' + ($errors -join '; ')) }
    foreach ($member in $members) { $script:Stopped.Remove($member.id) }
    $script:Report.restoration = 'restarted and ready'
    Save-Evidence
}

function Wait-NewLeader($Members, [long]$OldTerm, [long]$OldLeader) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $deadline = [DateTime]::UtcNow.AddSeconds($ElectionTimeoutSeconds)
    do {
        $views = @()
        foreach ($member in $Members) {
            $reply = Send-Fencing $member.url '/queue/raft' GET $null 'election' 'fault' 1500
            if ($reply.status -eq 200 -and $null -ne $reply.response) { $views += $reply.response }
        }
        if ($views.Count -eq $Members.Count) {
            $leaders = @($views | ForEach-Object { Get-Field $_ 'current_leader' } | Sort-Object -Unique)
            $terms = @($views | ForEach-Object { Get-Field $_ 'current_term' })
            if ($leaders.Count -eq 1 -and $null -ne $leaders[0] -and
                $leaders[0] -ne $OldLeader -and $leaders[0] -in @($Members.node_id) -and
                @($terms | Where-Object { $_ -le $OldTerm }).Count -eq 0 -and
                [DateTime]::UtcNow -le $deadline) {
                $script:Report.recovery_intervals.Add(@{ operation = 'leader_election'; milliseconds = $clock.Elapsed.TotalMilliseconds })
                Assert-Fencing $true 'FENCE-03 replacement leader in higher term with two members' @{
                    previous_leader = $OldLeader; previous_term = $OldTerm; views = $views
                }
                return @($Members | Where-Object { $_.node_id -eq $leaders[0] })[0]
            }
        }
        Start-Sleep -Milliseconds 250
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "No higher-term replacement leader within ${ElectionTimeoutSeconds}s."
}

function Wait-PendingRecovery([string]$Base, [string[]]$Sessions) {
    $clock = [Diagnostics.Stopwatch]::StartNew()
    $deadline = [DateTime]::UtcNow.AddSeconds($RecoveryTimeoutSeconds)
    do {
        $pending = @()
        $pendingIds = @()
        foreach ($sid in $Sessions) {
            $reply = Send-Fencing $Base "/calls/$sid/ownership" GET $null 'pending-recovery' 'recovery' 2000
            if ($reply.status -ne 200 -or $null -eq $reply.response -or
                $reply.response.PSObject.Properties.Name -notcontains 'pending_operation_id' -or
                $null -ne (Get-Field $reply.response 'pending_operation_id')) {
                $pending += $sid
                $operationId = Get-Field $reply.response 'pending_operation_id'
                if ($operationId) { $pendingIds += $operationId }
            }
        }
        $script:Report.unresolved_sessions = $pending
        if ($pending.Count -gt 0) {
            $script:Report.unresolved_operation_ids = @(
                @($script:Report.unresolved_operation_ids) + $pendingIds | Sort-Object -Unique
            )
        } else { $script:Report.unresolved_operation_ids = @() }
        if ($pending.Count -eq 0 -and [DateTime]::UtcNow -le $deadline) {
            $script:Report.recovery_intervals.Add(@{ operation = 'pending_reconciliation'; milliseconds = $clock.Elapsed.TotalMilliseconds })
            return
        }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Pending journal recovery exceeded ${RecoveryTimeoutSeconds}s."
}

function Wait-Convergence($Members, [string]$Session, $Volunteer, [long]$Generation, [long]$TargetIndex) {
    $deadline = [DateTime]::UtcNow.AddSeconds($RecoveryTimeoutSeconds)
    do {
        $views = @()
        $ok = $true
        foreach ($member in $Members) {
            $reply = Send-Fencing $member.url '/queue/raft' GET $null 'convergence' 'recovery' 2000
            $view = $reply.response
            $ownership = Get-Field (Get-Field $view 'ownership') $Session
            $applied = Get-Field $view 'last_applied'
            if ($reply.status -ne 200 -or (Get-Field $view 'backend') -ne 'raft' -or
                $null -eq $ownership -or $null -eq $applied -or $applied -lt $TargetIndex -or
                (Get-Field $ownership 'volunteer_id') -ne $Volunteer -or
                (Get-Field $ownership 'assignment_generation') -ne $Generation) { $ok = $false }
            $views += $view
        }
        $leaders = @($views | ForEach-Object { Get-Field $_ 'current_leader' } | Sort-Object -Unique)
        if ($ok -and $leaders.Count -eq 1 -and $null -ne $leaders[0] -and [DateTime]::UtcNow -le $deadline) {
            Assert-Fencing $true 'FENCE-05 ownership and target applied index converge on all members' @{
                session_id = $Session; generation = $Generation; target_applied_index = $TargetIndex; views = $views
            }
            return
        }
        Start-Sleep -Milliseconds 500
    } while ([DateTime]::UtcNow -lt $deadline)
    throw "Ownership/log catch-up exceeded ${RecoveryTimeoutSeconds}s."
}

function Test-Faults($Members) {
    $initial = Get-RaftStatus $Members[0].url
    $oldLeader = @($Members | Where-Object { $_.node_id -eq $initial.current_leader })[0]
    $survivors = @($Members | Where-Object { $_.id -ne $oldLeader.id })
    $sid = New-SyntheticCase $oldLeader.url 'raft'
    Assert-Transition (Claim-Case $oldLeader.url $sid 'volunteer-aisha') 'raft' $sid 'volunteer-aisha' 1 'fault fixture claim'
    $unclaimed = New-SyntheticCase $oldLeader.url 'raft'
    $script:FaultSessions = @($sid, $unclaimed)
    Stop-OwnedMember $oldLeader
    $leader = Wait-NewLeader $survivors $initial.current_term $initial.current_leader
    Assert-Transition (Act-Case $leader.url $sid 'volunteer-aisha' 1 'transfer' 'volunteer-noor') 'raft' $sid 'volunteer-noor' 2 'FENCE-03 higher-term reassignment'
    $second = @($survivors | Where-Object { $_.id -ne $leader.id })[0]
    Stop-OwnedMember $second
    $failedClaim = Claim-Case $leader.url $unclaimed 'volunteer-aisha' 'fault' '' ($UnavailableBoundSeconds * 1000)
    $script:Report.unresolved_operation_ids = @($failedClaim.request.request_id)
    Expect-Status $failedClaim 503 'FENCE-04 no quorum claim fails closed (timeout is not a pass)'
    Assert-Fencing ($failedClaim.milliseconds -le $UnavailableBoundSeconds * 1000) 'no quorum claim satisfies predeclared bound' $failedClaim.milliseconds
    $failedTransfer = Act-Case $leader.url $sid 'volunteer-noor' 2 'transfer' 'volunteer-aisha' 'fault' '' ($UnavailableBoundSeconds * 1000)
    $script:Report.unresolved_operation_ids += $failedTransfer.request.request_id
    Expect-Status $failedTransfer 503 'FENCE-04 no quorum transfer fails closed'
    Assert-Fencing ($failedTransfer.milliseconds -le $UnavailableBoundSeconds * 1000) 'no quorum transfer satisfies predeclared bound' $failedTransfer.milliseconds
    foreach ($case in @(@{ id = $unclaimed; owner = $null; generation = 0 }, @{ id = $sid; owner = 'volunteer-noor'; generation = 2 })) {
        $owned = Get-Ownership $leader.url $case.id
        Assert-Fencing ((Get-Field $owned 'volunteer_id') -eq $case.owner -and
            (Get-Field $owned 'assignment_generation') -eq $case.generation -and
            (Get-Field $owned 'coordinator') -eq 'raft') 'minority cannot fall back to PostgreSQL ownership' $owned
    }
    Restore-OwnedMembers
    Wait-Readiness $Members
    # Do not retry the ambiguous requests: observe the background journal reconciler.
    Wait-PendingRecovery $leader.url @($sid, $unclaimed)
    $current = Get-Ownership $leader.url $sid
    $generation = [long]$current.assignment_generation
    $owner = $current.volunteer_id
    Assert-Fencing (($generation -eq 2 -and $owner -eq 'volunteer-noor') -or
        ($generation -eq 3 -and $owner -eq 'volunteer-aisha')) 'ambiguous transfer recovered to an allowed one-effect outcome' $current
    $targetVolunteer = 'volunteer-aisha'
    if ($owner -eq 'volunteer-aisha') { $targetVolunteer = 'volunteer-noor' }
    $completed = Act-Case $leader.url $sid $owner $generation 'transfer' $targetVolunteer
    Assert-Transition $completed 'raft' $sid $targetVolunteer ($generation + 1) 'post-restoration ownership transition'
    $status = Get-RaftStatus $leader.url
    $currentLeader = @($Members | Where-Object { $_.node_id -eq $status.current_leader })
    Assert-Fencing ($currentLeader.Count -eq 1) 'target log index comes from an actual current leader' $status
    $status = Get-RaftStatus $currentLeader[0].url
    $appliedOwnership = Get-Field (Get-Field $status 'ownership') $sid
    Assert-Fencing ((Get-Field $appliedOwnership 'volunteer_id') -eq $targetVolunteer -and
        (Get-Field $appliedOwnership 'assignment_generation') -eq ($generation + 1)) 'target leader has applied the acknowledged ownership transition' $status
    Assert-Fencing ($null -ne $status.last_applied) 'target log index is available' $status
    $script:ConvergenceTarget = @{ session = $sid; volunteer = $targetVolunteer; generation = $generation + 1; index = [long]$status.last_applied }
    $script:Report.target = $script:ConvergenceTarget
    Wait-Convergence $Members $sid $targetVolunteer ($generation + 1) ([long]$status.last_applied)
    $script:Report.faults.raft = 'passed'
}

function Invoke-SqlAudit {
    if (-not $AuditPostgresContainer) { return }
    foreach ($value in @($AuditPostgresContainer, $AuditDatabase, $AuditUser)) {
        if ($value -notmatch '^[A-Za-z0-9_.-]+$') { throw 'Audit identifiers must contain only letters, digits, dot, underscore or hyphen.' }
    }
    $ids = @($script:Report.sessions | ForEach-Object { "'" + $_.session_id + "'" }) -join ','
    if (-not $ids) { throw 'No sessions available to audit.' }
    $sql = @"
WITH selected AS (SELECT * FROM case_ownership WHERE session_id IN ($ids))
SELECT json_build_object(
 'ownership', (SELECT coalesce(json_agg(o), '[]'::json) FROM selected o),
 'history', (SELECT coalesce(json_agg(a), '[]'::json) FROM volunteer_assignments a WHERE session_id IN ($ids)),
 'notes', (SELECT coalesce(json_agg(n), '[]'::json) FROM volunteer_case_notes n WHERE session_id IN ($ids)),
 'pending', (SELECT coalesce(json_agg(p), '[]'::json) FROM assignment_operations p WHERE session_id IN ($ids) AND completed_at IS NULL),
 'duplicate_active', (SELECT coalesce(json_agg(d), '[]'::json) FROM
   (SELECT session_id, count(*) FROM volunteer_assignments WHERE session_id IN ($ids) AND resolved_at IS NULL GROUP BY session_id HAVING count(*) > 1) d));
"@
    $text = Invoke-Docker @('exec', '-i', $AuditPostgresContainer, 'psql', '-X', '-w',
        '-v', 'ON_ERROR_STOP=1', '-U', $AuditUser, '-d', $AuditDatabase, '-A', '-t') $sql
    $audit = $text | ConvertFrom-Json
    $script:Report.sql_audit = @{ status = 'executed'; container = $AuditPostgresContainer; result = $audit }
    Assert-Fencing (@($audit.duplicate_active).Count -eq 0) 'SQL no duplicate active durable assignments' $audit.duplicate_active
    Assert-Fencing (@($audit.pending).Count -eq 0) 'SQL no unresolved journal operations' $audit.pending
    foreach ($session in $script:Report.sessions) {
        $rows = @($audit.ownership | Where-Object { $_.session_id -eq $session.session_id })
        Assert-Fencing ($rows.Count -eq 1) 'SQL audit includes every synthetic case (correct database)' $session
        $ownership = Get-Ownership $session.base_url $session.session_id
        Assert-Fencing ($rows[0].assignment_generation -eq $ownership.assignment_generation -and
            $rows[0].volunteer_id -eq $ownership.volunteer_id) 'SQL ownership matches HTTP projection' $rows[0]
    }
    $rejectedIds = @($script:Report.requests | Where-Object { $_.operation -eq 'notes' -and $_.status -eq 409 } |
        ForEach-Object { $_.request.request_id })
    foreach ($note in @($audit.notes)) {
        Assert-Fencing ($note.request_id -notin $rejectedIds) 'SQL stale note never persisted' $note
    }
    foreach ($request in @($script:Report.requests | Where-Object { $_.operation -eq 'notes' -and $_.status -eq 200 })) {
        Assert-Fencing (@($audit.notes | Where-Object { $_.request_id -eq $request.request.request_id }).Count -eq 1) 'SQL accepted note persisted exactly once' $request.request.request_id
    }
}

if ($LibraryOnly) { return }

$script:Report = $null
$script:Stopped = @{}
$script:ConvergenceTarget = $null
$script:FaultSessions = @()
$script:Members = @()
$exitCode = 1
$runId = [DateTime]::UtcNow.ToString('yyyyMMddTHHmmssfffZ') + '-' + [Guid]::NewGuid().ToString('N').Substring(0, 8)
$artifactRoot = Join-Path (Join-Path $PSScriptRoot '..') 'artifacts\fencing'
$script:EvidenceDirectory = Join-Path $artifactRoot $runId
$null = New-Item -ItemType Directory -Path $script:EvidenceDirectory -Force
$script:Report = [ordered]@{
    run_id = $runId; started_utc = [DateTime]::UtcNow.ToString('o'); updated_utc = $null; finished_utc = $null
    outcome = 'running'; exit_code = $null; errors = (New-Object 'System.Collections.Generic.List[string]')
    git_revision = $null
    configuration = @{
        raft_urls = $RaftUrls; postgres_url = $PostgresUrl; skip_postgres = [bool]$SkipPostgres
        latency_only = [bool]$LatencyOnly; inject_faults = [bool]$InjectFaults
        samples = $SampleCount; warmup = $WarmupCount; latency_concurrency = 1; race_concurrency = 2
        http_timeout_seconds = $HttpTimeoutSeconds; unavailable_bound_seconds = $UnavailableBoundSeconds
        election_timeout_seconds = $ElectionTimeoutSeconds; recovery_timeout_seconds = $RecoveryTimeoutSeconds
        stop_grace_seconds = $StopTimeoutSeconds; fault_sequence = 'stop leader, elect with 2/3, stop follower, restore both'
        ai_policy = 'Fresh SIM case; exactly one built-in crisis keyword turn; immediate escalation bypasses inference'
        raft_database_topology = $RaftDatabaseTopology; postgres_database_topology = $PostgresDatabaseTopology
        image_identity = 'Unverified unless fault-mode Compose inspection runs; custom local processes need not use images'
    }
    comparison = 'Deployment comparison only; Docker Raft and port-18080 topology/database replication may differ. Not isolated Raft overhead.'
    preflight = (New-Object 'System.Collections.Generic.List[object]')
    sessions = (New-Object 'System.Collections.Generic.List[object]')
    assertions = (New-Object 'System.Collections.Generic.List[object]')
    requests = (New-Object 'System.Collections.Generic.List[object]')
    native_commands = (New-Object 'System.Collections.Generic.List[object]')
    api_exposed_sql_audits = (New-Object 'System.Collections.Generic.List[object]')
    recovery_intervals = (New-Object 'System.Collections.Generic.List[object]')
    latency = @{}; containers = @(); target = $null; unresolved_sessions = @(); unresolved_operation_ids = @()
    faults = @{ raft = 'not run (requires -InjectFaults and approval)'; postgres = 'N/A: no Raft election or quorum in PostgreSQL mode' }
    sql_audit = @{ status = 'Independent admin SQL NOT RUN: specify -AuditPostgresContainer; API-exposed PostgreSQL query evidence is separately labelled' }
    not_covered = @('Database-failure-after-commit injection', 'counter exhaustion and legacy migration', 'API replica failure', 'physical-machine isolation')
    restoration = 'not needed'
}
Save-Evidence
try {
    if ($LatencyOnly -and $InjectFaults) { throw '-LatencyOnly and -InjectFaults are mutually exclusive.' }
    if ($RaftUrls.Count -ne 3) { throw 'Exactly three Raft URLs are required.' }
    $script:RaftBases = @($RaftUrls | ForEach-Object { ConvertTo-ApiUrl $_ })
    if (@($script:RaftBases | Sort-Object -Unique).Count -ne 3) { throw 'Raft URLs must be distinct.' }
    $postgresBase = ConvertTo-ApiUrl $PostgresUrl
    if (-not $SkipPostgres -and $postgresBase -in $script:RaftBases) { throw 'Comparison URL must not be a Raft URL.' }
    if ($SkipPostgres) { $script:Report.comparison = 'N/A: PostgreSQL comparison explicitly skipped; no relative latency claim.' }
    $revision = & git --no-pager rev-parse HEAD 2>$null
    $script:Report.native_commands.Add(@{ executable = 'git'; arguments = @('rev-parse', 'HEAD'); exit_code = $LASTEXITCODE })
    if ($LASTEXITCODE -eq 0) { $script:Report.git_revision = "$revision".Trim() }
    Initialize-FencingTransport
    Test-Preflight $script:RaftBases 'raft'
    if (-not $SkipPostgres) { Test-Preflight @($postgresBase) 'postgres' }
    if (-not $LatencyOnly) {
        Test-Correctness $script:RaftBases 'raft'
        if (-not $SkipPostgres) { Test-Correctness @($postgresBase) 'postgres' }
    }
    Test-Latency $script:RaftBases[0] 'raft'
    if (-not $SkipPostgres) { Test-Latency $postgresBase 'postgres' }
    if ($InjectFaults) {
        $script:Members = @(Get-ComposeMembers)
        $description = 'Stop the current leader, then one follower; restore only those exact container IDs'
        if (-not $PSCmdlet.ShouldProcess(($script:Members.id -join ', '), $description)) {
            $script:Report.faults.raft = 'NOT RUN: ShouldProcess declined'
            throw 'Requested fault acceptance was not executed: approval declined/WhatIf.'
        }
        Test-Faults $script:Members
    }
    Invoke-SqlAudit
    $exitCode = 0
} catch {
    $script:Report.errors.Add($_.Exception.Message)
} finally {
    # Always attempt every owned restart, even if an earlier assertion/CLI failed.
    if ($script:Stopped.Count -gt 0) {
        try { Restore-OwnedMembers } catch {
            $script:Report.errors.Add($_.Exception.Message)
            $script:Report.restoration = 'FAILED; inspect errors and remaining stopped containers'
            $exitCode = 1
        }
    }
    if ($script:Members.Count -gt 0) {
        try {
            Wait-Readiness $script:Members
            Test-Preflight $script:RaftBases 'raft'
            if ($script:FaultSessions.Count -gt 0) {
                Wait-PendingRecovery $script:RaftBases[0] $script:FaultSessions
            }
            if ($null -ne $script:ConvergenceTarget) {
                $target = $script:ConvergenceTarget
                Wait-Convergence $script:Members $target.session $target.volunteer $target.generation $target.index
            } else {
                # An aborted scenario may have committed an ambiguous request.
                # Read its recovered durable state, then verify every Raft projection.
                $leaderId = (Get-RaftStatus $script:RaftBases[0]).current_leader
                $leader = @($script:Members | Where-Object { $_.node_id -eq $leaderId })[0]
                foreach ($sid in $script:FaultSessions) {
                    $ownership = Get-Ownership $leader.url $sid
                    if ($ownership.assignment_generation -gt 0) {
                        $status = Get-RaftStatus $leader.url
                        Assert-Fencing ($null -ne $status.last_applied) 'restoration target applied index exists' $status
                        Wait-Convergence $script:Members $sid $ownership.volunteer_id $ownership.assignment_generation $status.last_applied
                    }
                }
            }
        } catch {
            $script:Report.errors.Add('Final readiness/catch-up: ' + $_.Exception.Message)
            $exitCode = 1
        }
    }
    foreach ($mode in @('raft', 'postgres')) {
        $bases = @($script:Report.preflight | Where-Object { $_.coordinator -eq $mode } | ForEach-Object { $_.url })
        $requests = @($script:Report.requests | Where-Object {
            $requestBase = $_.url.Substring(0, $_.url.IndexOf('/api/v1') + 7)
            $requestBase -in $bases
        })
        if ($bases.Count -gt 0) { $script:Report.latency[$mode] = @(Get-LatencyStatistics $requests) }
    }
    $script:Report.remaining_stopped_containers = @($script:Stopped.Keys)
    $script:Report.finished_utc = [DateTime]::UtcNow.ToString('o')
    $script:Report.exit_code = $exitCode
    $script:Report.outcome = 'failed'
    if ($exitCode -eq 0) { $script:Report.outcome = 'passed executed scenarios (see NOT RUN/N/A coverage)' }
    Save-Evidence
}
foreach ($mode in @('raft', 'postgres')) {
    if ($script:Report.latency.ContainsKey($mode)) {
        Write-Host "`n$mode latency (successful requests only; failures counted separately)"
        $script:Report.latency[$mode] | Format-Table operation, attempted, successful, failures, median_ms, p95_ms -AutoSize | Out-Host
    }
}
Write-Host $script:Report.comparison
Write-Host "Evidence: $script:EvidenceDirectory"
foreach ($message in $script:Report.errors) { Write-Warning $message }
exit $exitCode
