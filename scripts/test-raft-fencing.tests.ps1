#requires -Version 5.1
# Offline parser/pure-function checks. Does not contact APIs or Docker.
param([switch]$WithMockServer)
$ErrorActionPreference = 'Stop'
$scriptPath = Join-Path $PSScriptRoot 'test-raft-fencing.ps1'
$tokens = $null
$parseErrors = $null
$ast = [System.Management.Automation.Language.Parser]::ParseFile($scriptPath, [ref]$tokens, [ref]$parseErrors)
if ($parseErrors.Count -gt 0) { throw ($parseErrors | Out-String) }
. $scriptPath -LibraryOnly

function Check([bool]$Condition, [string]$Name) {
    if (-not $Condition) { throw "FAILED: $Name" }
    Write-Host "PASS: $Name"
}

Check ((ConvertTo-ApiUrl 'http://localhost:18081/') -eq 'http://localhost:18081/api/v1') 'root URL normalization'
Check ((ConvertTo-ApiUrl 'http://localhost:18081/api/v1/') -eq 'http://localhost:18081/api/v1') 'API URL normalization'
foreach ($bad in @('ftp://localhost:18081', 'http://user:password@localhost', 'http://localhost/api/v2', 'http://localhost/?secret=value')) {
    $rejected = $false
    try { $null = ConvertTo-ApiUrl $bad } catch { $rejected = $true }
    Check $rejected 'reject unsafe or mixed URL'
}
Check ($null -eq (Get-Field $null 'absent')) 'null diagnostic is not success-shaped'
Check ((Get-Field ([pscustomobject]@{ generation = 5 }) 'generation') -eq 5) 'diagnostic field extraction'

$samples = @(
    [pscustomobject]@{ phase = 'warmup'; operation = 'claim'; status = 200; milliseconds = 9999 },
    [pscustomobject]@{ phase = 'sample'; operation = 'claim'; status = 200; milliseconds = 10 },
    [pscustomobject]@{ phase = 'sample'; operation = 'claim'; status = 200; milliseconds = 20 },
    [pscustomobject]@{ phase = 'sample'; operation = 'claim'; status = 503; milliseconds = 8000 },
    [pscustomobject]@{ phase = 'sample'; operation = 'transfer'; status = 0; milliseconds = 10000 }
)
$stats = @(Get-LatencyStatistics $samples)
$claim = @($stats | Where-Object { $_.operation -eq 'claim' })[0]
$transfer = @($stats | Where-Object { $_.operation -eq 'transfer' })[0]
Check ($claim.attempted -eq 3 -and $claim.successful -eq 2 -and $claim.failures -eq 1) 'availability denominator retains failures and excludes warmup'
Check ($claim.median_ms -eq 15 -and $claim.p95_ms -eq 20) 'median and nearest-rank p95'
Check ($null -eq $transfer.median_ms -and $null -eq $transfer.p95_ms -and $transfer.timeouts_or_transport_errors -eq 1) 'all-failed operation has null percentiles'
Check ($stats.Count -eq 2) 'operation distributions remain separate'
Initialize-FencingTransport
Check ($null -ne ('FencingHarness.Transport' -as [type])) 'HTTP async barrier compiles on Windows PowerShell'
$commands = @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true))
Check (@($commands | Where-Object { $_.GetCommandName() -in @('Start-Job', 'Invoke-Expression') }).Count -eq 0) 'no jobs or evaluated shell commands'
$content = [IO.File]::ReadAllText($scriptPath)
Check ($content.Contains('RunContinuationsAsynchronously') -and $content.Contains('gate.SetResult(0)')) 'race uses shared asynchronous start barrier'
Check ($content.Contains('finally {') -and $content.Contains('Restore-OwnedMembers')) 'restoration is protected by finally'
Write-Host 'Offline fencing harness checks passed. No live fault tests executed.'

if ($WithMockServer) {
    Add-Type -AssemblyName System.Web.Extensions
    Add-Type -ReferencedAssemblies System.Web.Extensions -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.IO;
using System.Net;
using System.Net.Sockets;
using System.Text;
using System.Threading;
using System.Web.Script.Serialization;
namespace FencingHarnessTests {
    public sealed class Mock : IDisposable {
        private readonly List<HttpListener> listeners = new List<HttpListener>();
        private readonly Dictionary<string, Dictionary<string, object>> ownership =
            new Dictionary<string, Dictionary<string, object>>();
        private readonly Dictionary<string, string> operations = new Dictionary<string, string>();
        private readonly Dictionary<string, Dictionary<string, object>> outcomes =
            new Dictionary<string, Dictionary<string, object>>();
        private readonly Dictionary<string, List<Dictionary<string, object>>> history =
            new Dictionary<string, List<Dictionary<string, object>>>();
        private readonly Dictionary<string, List<Dictionary<string, object>>> notes =
            new Dictionary<string, List<Dictionary<string, object>>>();
        private readonly object sync = new object();
        public readonly List<int> Ports = new List<int>();
        public bool WrongCoordinator;
        public bool FailNotes;
        private bool disposed;
        public Mock() {
            for (int i = 0; i < 4; i++) {
                var reserve = new TcpListener(IPAddress.Loopback, 0);
                reserve.Start();
                int port = ((IPEndPoint)reserve.LocalEndpoint).Port;
                reserve.Stop();
                var listener = new HttpListener();
                listener.Prefixes.Add("http://localhost:" + port + "/");
                listener.Start();
                Ports.Add(port);
                listeners.Add(listener);
                int node = i;
                ThreadPool.QueueUserWorkItem(_ => Listen(listener, node));
            }
        }
        private void Listen(HttpListener listener, int node) {
            while (!disposed) {
                try {
                    var context = listener.GetContext();
                    ThreadPool.QueueUserWorkItem(_ => Handle(context, node));
                } catch (HttpListenerException) { return; }
                  catch (ObjectDisposedException) { return; }
            }
        }
        private void Handle(HttpListenerContext context, int node) {
            var serializer = new JavaScriptSerializer();
            try {
                object body;
                int code;
                using (var reader = new StreamReader(context.Request.InputStream)) {
                    string json = reader.ReadToEnd();
                    var input = string.IsNullOrEmpty(json) ? new Dictionary<string, object>() :
                        serializer.Deserialize<Dictionary<string, object>>(json);
                    lock (sync) {
                        body = Dispatch(context.Request.Url.AbsolutePath.Substring(7), input,
                            node, out code);
                    }
                }
                context.Response.StatusCode = code;
                context.Response.ContentType = "application/json";
                byte[] bytes = Encoding.UTF8.GetBytes(serializer.Serialize(body));
                context.Response.ContentLength64 = bytes.Length;
                context.Response.OutputStream.Write(bytes, 0, bytes.Length);
            } catch (Exception) { context.Response.StatusCode = 500; }
            finally { context.Response.Close(); }
        }
        private object Dispatch(string path, Dictionary<string, object> input,
                                int node, out int code) {
            code = 200;
            string mode = node == 3 ? "postgres" : "raft";
            if (WrongCoordinator) mode = mode == "raft" ? "postgres" : "raft";
            if (path == "/health" || path == "/ready") return new { status = "ok" };
            if (path == "/queue/raft") return new {
                backend = node == 3 ? "redis" : "raft", node_id = node + 1,
                current_leader = 1, current_term = 1, last_applied = 10000,
                ownership = ownership
            };
            if (path == "/calls/incoming") return new { session_id = input["call_sid"], state = "incoming" };
            var parts = path.Trim('/').Split('/');
            if (parts.Length == 4 && parts[0] == "calls" && parts[3] == "audit")
                return new { session_id = parts[1], history = history[parts[1]], notes = notes[parts[1]] };
            if (parts.Length == 3 && parts[0] == "calls") {
                if (parts[2] == "turn") return new { state = "escalated" };
                if (parts[2] == "ownership") {
                    Dictionary<string, object> current;
                    if (ownership.TryGetValue(parts[1], out current)) return current;
                    return new { volunteer_id = (string)null, assignment_generation = 0,
                        pending_operation_id = (string)null, coordinator = mode };
                }
            }
            bool claim = path == "/volunteers/claim";
            string sid = claim ? (string)input["session_id"] : parts[3];
            string volunteer = claim ? (string)input["volunteer_id"] : parts[1];
            string action = claim ? "claim" : parts[4];
            string request = (string)input["request_id"];
            string target = input.ContainsKey("to_volunteer_id") ? (string)input["to_volunteer_id"] : null;
            if ((volunteer != "volunteer-aisha" && volunteer != "volunteer-noor") ||
                (target != null && target != "volunteer-aisha" && target != "volunteer-noor")) {
                code = 400; return new { error = "off-shift volunteer" };
            }
            long expected = claim ? 0 : Convert.ToInt64(input["assignment_generation"]);
            string fingerprint = sid + "|" + volunteer + "|" + action + "|" + expected + "|" + target;
            if (operations.ContainsKey(request)) {
                if (operations[request] != fingerprint) { code = 409; return new { error = "request conflict" }; }
                return outcomes[request];
            }
            Dictionary<string, object> state;
            if (!ownership.TryGetValue(sid, out state)) {
                state = new Dictionary<string, object> {
                    {"volunteer_id", null}, {"assignment_generation", 0L},
                    {"pending_operation_id", null}, {"coordinator", mode}
                };
                ownership[sid] = state;
                history[sid] = new List<Dictionary<string, object>>();
                notes[sid] = new List<Dictionary<string, object>>();
            }
            string owner = (string)state["volunteer_id"];
            long generation = Convert.ToInt64(state["assignment_generation"]);
            if ((claim && owner != null) || (!claim && (expected != generation || owner != volunteer))) {
                code = 409; return new { error = "stale/conflict" };
            }
            if (action == "notes" && FailNotes) { code = 503; return new { error = "mock unavailable" }; }
            if (action != "notes") {
                foreach (var row in history[sid]) {
                    if ((bool)row["active"]) { row["active"] = false; row["status"] = action; }
                }
                state["volunteer_id"] = claim ? volunteer : action == "transfer" ? target : null;
                state["assignment_generation"] = generation + 1;
                if (state["volunteer_id"] != null) history[sid].Add(new Dictionary<string, object> {
                    {"volunteer_id", state["volunteer_id"]}, {"assignment_generation", generation + 1},
                    {"status", "assigned"}, {"active", true}
                });
            } else {
                notes[sid].Add(new Dictionary<string, object> {
                    {"request_id", request}, {"volunteer_id", volunteer}, {"assignment_generation", generation}
                });
            }
            var result = new Dictionary<string, object>(state) {
                {"session_id", sid}, {"request_id", request}
            };
            operations[request] = fingerprint;
            outcomes[request] = result;
            return result;
        }
        public void Dispose() {
            disposed = true;
            foreach (var listener in listeners) listener.Close();
        }
    }
}
'@
    $mock = New-Object FencingHarnessTests.Mock
    function Invoke-MockHarness([string]$ExtraArguments, [int]$ExpectedExit) {
        $process = New-Object System.Diagnostics.Process
        try {
        $urls = @($mock.Ports | ForEach-Object { 'http://localhost:' + $_ })
        $quotedScript = $scriptPath.Replace("'", "''")
        $command = "`$ProgressPreference = 'SilentlyContinue'; & '$quotedScript' -RaftUrls @('$($urls[0])','$($urls[1])','$($urls[2])') " +
            "-PostgresUrl '$($urls[3])' -SampleCount 1 -WarmupCount 0 " +
            "-RaftDatabaseTopology 'LOCAL MOCK: no database, no live acceptance evidence' " +
            "-PostgresDatabaseTopology 'LOCAL MOCK: no database, no live acceptance evidence' $ExtraArguments"
        $process.StartInfo.FileName = 'powershell.exe'
        $process.StartInfo.Arguments = '-NoProfile -OutputFormat Text -ExecutionPolicy Bypass -EncodedCommand ' +
            [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($command))
        $process.StartInfo.UseShellExecute = $false
        $process.StartInfo.RedirectStandardOutput = $true
        $process.StartInfo.RedirectStandardError = $true
        $null = $process.Start()
        $stdout = $process.StandardOutput.ReadToEndAsync()
        $stderr = $process.StandardError.ReadToEndAsync()
        if (-not $process.WaitForExit(120000)) {
            Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
            throw 'Local mock integration timed out.'
        }
        $output = $stdout.GetAwaiter().GetResult()
        $errorOutput = $stderr.GetAwaiter().GetResult()
        if ($process.ExitCode -ne $ExpectedExit) {
            Write-Host $output
            Write-Host $errorOutput
        }
        Check ($process.ExitCode -eq $ExpectedExit) "mock harness exits $ExpectedExit"
        $match = [regex]::Match($output, 'Evidence:\s*([^\r\n]+)')
        Check $match.Success 'run reports persistent evidence location'
        $directory = $match.Groups[1].Value.Trim()
        $report = Get-Content -LiteralPath (Join-Path $directory 'report.json') -Raw | ConvertFrom-Json
        Check ($report.exit_code -eq $ExpectedExit) 'JSON preserves actual process exit'
        return $report
        } finally { $process.Dispose() }
    }
    try {
        $report = Invoke-MockHarness '' 0
        $claimStats = @($report.latency.raft | Where-Object { $_.operation -eq 'claim' })[0]
        Check ($claimStats.attempted -eq 1) 'complete harness latency sample count'
        Check (@($report.assertions | Where-Object { -not $_.passed }).Count -eq 0) 'race, stale writes, ABA, release/reclaim and both coordinators'
        $mock.WrongCoordinator = $true
        $report = Invoke-MockHarness '-SkipPostgres' 1
        Check (@($report.sessions).Count -eq 0) 'wrong coordinator fails before creating cases'
        $mock.WrongCoordinator = $false
        $mock.FailNotes = $true
        $report = Invoke-MockHarness '-SkipPostgres -LatencyOnly' 1
        $noteStats = @($report.latency.raft | Where-Object { $_.operation -eq 'notes' })[0]
        Check ($noteStats.failures -eq 1 -and $null -eq $noteStats.p95_ms) 'latency failure preserved in evidence without success percentile'
        Check ($report.comparison -like 'N/A:*') 'skipped comparison is N/A'
    } finally {
        $mock.Dispose()
    }
}
