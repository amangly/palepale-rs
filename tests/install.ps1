$ErrorActionPreference = 'Stop'
$installer = Join-Path $PSScriptRoot '../data/install.ps1'
$global:palepaleTestpayload = [Text.Encoding]::UTF8.GetBytes('local fixture, never executed')
$sha = [Security.Cryptography.SHA256]::Create()
$global:palepaleTesthash = ([BitConverter]::ToString($sha.ComputeHash($global:palepaleTestpayload))).Replace('-', '')
$sha.Dispose()
$global:palepaleTestdirectories = @()
$global:palepaleTestcommands = @()
$global:palepaleTestscenario = ''
$global:palepaleTestinstalled = $false
$global:palepaleTestrequested = $false

function Invoke-WebRequest {
    param([switch]$UseBasicParsing, $Uri, $OutFile)
    $global:palepaleTestrequested = $true
    $global:palepaleTestdirectories += Split-Path $OutFile
    if ($global:palepaleTestscenario -eq 'download_failure') { throw 'fixture download failure' }
    if ($Uri.EndsWith('/SHA256SUMS')) {
        $line = "$global:palepaleTesthash  palepale-x86_64-pc-windows-msvc.exe"
        switch ($global:palepaleTestscenario) {
            'missing' { $line = '' }
            'duplicate' { $line = "$line`n$line" }
            'corrupt' { $line = ('0' * 64) + '  palepale-x86_64-pc-windows-msvc.exe' }
        }
        [IO.File]::WriteAllText($OutFile, $line)
    } else {
        if (-not $Uri.EndsWith('/palepale-x86_64-pc-windows-msvc.exe')) { throw "Wrong asset: $Uri" }
        [IO.File]::WriteAllBytes($OutFile, $global:palepaleTestpayload)
        # Resolve the downloaded path to a function so no binary is executed.
        Set-Item -Path "Function:global:$OutFile" -Value {
            param($command)
            if ($command -ne 'install') { throw "Wrong command: $command" }
            if ($global:palepaleTestscenario -eq 'install_failure') { $global:LASTEXITCODE = 10; return }
            $global:palepaleTestinstalled = $true
            $global:LASTEXITCODE = 0
        }
        $global:palepaleTestcommands += $OutFile
    }
}

$originalArch = $env:PROCESSOR_ARCHITECTURE
$originalWow = $env:PROCESSOR_ARCHITEW6432
try {
    foreach ($case in @('success', 'wow64', 'arm', 'corrupt', 'missing', 'duplicate', 'download_failure', 'install_failure')) {
        $global:palepaleTestscenario = $case
        $global:palepaleTestinstalled = $false
        $global:palepaleTestrequested = $false
        $env:PROCESSOR_ARCHITECTURE = 'AMD64'
        $env:PROCESSOR_ARCHITEW6432 = ''
        if ($case -eq 'wow64') {
            $env:PROCESSOR_ARCHITECTURE = 'x86'
            $env:PROCESSOR_ARCHITEW6432 = 'AMD64'
        }
        if ($case -eq 'arm') { $env:PROCESSOR_ARCHITECTURE = 'ARM64' }
        $failed = $false
        $failure = ''
        try { & $installer } catch { $failed = $true; $failure = $_.ToString() }
        $success = $case -in @('success', 'wow64')
        if ($failed -eq $success -or $global:palepaleTestinstalled -ne $success) {
            throw "Unexpected result for $case (failed=$failed, installed=$global:palepaleTestinstalled): $failure"
        }
        if ($case -eq 'arm' -and $global:palepaleTestrequested) { throw 'Unsupported platform downloaded files.' }
        foreach ($directory in $global:palepaleTestdirectories) {
            if (Test-Path -LiteralPath $directory) { throw "Temporary files remain: $directory" }
        }
    }
    Write-Host 'PowerShell installer: 8 fixture scenarios passed.'
} finally {
    $env:PROCESSOR_ARCHITECTURE = $originalArch
    $env:PROCESSOR_ARCHITEW6432 = $originalWow
    foreach ($command in $global:palepaleTestcommands) { Remove-Item -LiteralPath "Function:$command" }
}
