$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'recorded-process.ps1')
$directory = Join-Path ([System.IO.Path]::GetTempPath()) ("sayangcare-process-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $directory | Out-Null
try {
    foreach ($expected in @(0, 99)) {
        $stdout = Join-Path $directory "$expected.out"
        $stderr = Join-Path $directory "$expected.err"
        $recording = Start-RecordedProcess -FilePath $env:ComSpec `
            -Arguments @('/d', '/c', "`"echo stdout& echo stderr 1>&2& exit $expected`"") `
            -WorkingDirectory $directory -StdoutPath $stdout -StderrPath $stderr
        while (-not $recording.Process.HasExited) { Start-Sleep -Milliseconds 10 }
        $actual = Complete-RecordedProcess $recording
        if ($actual -ne $expected) { throw "Expected exit $expected, got $actual." }
        if ((Get-Content -Raw $stdout).Trim() -ne 'stdout') { throw 'stdout was not captured.' }
        if ((Get-Content -Raw $stderr).Trim() -ne 'stderr') { throw 'stderr was not captured.' }
    }
    Write-Host 'PASS: native exits 0 and 99, separate output streams, polling and completion.'
} finally {
    foreach ($file in @('0.out', '0.err', '99.out', '99.err')) {
        $path = Join-Path $directory $file
        if (Test-Path $path) { Remove-Item -LiteralPath $path }
    }
    Remove-Item -LiteralPath $directory
}
