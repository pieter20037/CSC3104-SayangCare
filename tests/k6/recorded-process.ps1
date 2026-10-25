function Start-RecordedProcess {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$WorkingDirectory,
        [string]$StdoutPath,
        [string]$StderrPath
    )

    $info = New-Object System.Diagnostics.ProcessStartInfo
    $info.FileName = (Get-Command $FilePath -ErrorAction Stop).Source
    $info.Arguments = $Arguments -join ' '
    $info.WorkingDirectory = $WorkingDirectory
    $info.UseShellExecute = $false
    $info.CreateNoWindow = $true
    $info.RedirectStandardOutput = $true
    $info.RedirectStandardError = $true
    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $info
    $stdout = [System.IO.File]::Create($StdoutPath)
    $stderr = $null
    $started = $false
    try {
        $stderr = [System.IO.File]::Create($StderrPath)
        if (-not $process.Start()) { throw "Could not start $FilePath." }
        $started = $true
        return [pscustomobject]@{
            Process = $process
            Stdout = $stdout
            Stderr = $stderr
            StdoutCopy = $process.StandardOutput.BaseStream.CopyToAsync($stdout)
            StderrCopy = $process.StandardError.BaseStream.CopyToAsync($stderr)
        }
    } catch {
        if ($started -and -not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
            $process.WaitForExit()
        }
        $stdout.Dispose()
        if ($null -ne $stderr) { $stderr.Dispose() }
        $process.Dispose()
        throw
    }
}

function Complete-RecordedProcess {
    param($Recording)

    try {
        $Recording.Process.WaitForExit()
        $null = $Recording.StdoutCopy.GetAwaiter().GetResult()
        $null = $Recording.StderrCopy.GetAwaiter().GetResult()
        $code = $Recording.Process.ExitCode
        if ($null -eq $code) { throw 'Native process exit code is unavailable.' }
        return [int]$code
    } finally {
        $Recording.Stdout.Dispose()
        $Recording.Stderr.Dispose()
        $Recording.Process.Dispose()
    }
}
