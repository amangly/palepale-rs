# A function keeps a truncated piped download from starting installation.
function Install-Palepale {
    $ErrorActionPreference = 'Stop'
    if ($env:OS -ne 'Windows_NT') {
        throw 'Use data/install.sh on macOS or Linux.'
    }
    $architecture = $env:PROCESSOR_ARCHITEW6432
    if (-not $architecture) { $architecture = $env:PROCESSOR_ARCHITECTURE }
    if ($architecture -ne 'AMD64') {
        throw "Windows requires x64; found $architecture."
    }
    $asset = 'palepale-x86_64-pc-windows-msvc.exe'
    $base = 'https://api.palepale.tech/cli/latest'
    $temp = Join-Path ([IO.Path]::GetTempPath()) ('palepale-install-' + [guid]::NewGuid())
    $null = New-Item -ItemType Directory -Path $temp
    $previousTls = [Net.ServicePointManager]::SecurityProtocol
    try {
        [Net.ServicePointManager]::SecurityProtocol = $previousTls -bor [Net.SecurityProtocolType]::Tls12
        Write-Host "Downloading $asset..."
        $manifest = Join-Path $temp 'SHA256SUMS'
        $binary = Join-Path $temp 'palepale.exe'
        Invoke-WebRequest -UseBasicParsing -Uri "$base/SHA256SUMS" -OutFile $manifest
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$asset" -OutFile $binary
        $pattern = '^([0-9a-fA-F]{64})\s+\*?' + [regex]::Escape($asset) + '$'
        $hashes = @(Get-Content -LiteralPath $manifest | ForEach-Object {
            if ($_ -match $pattern) { $Matches[1] }
        })
        if ($hashes.Count -ne 1) { throw 'Missing or ambiguous release checksum.' }
        if ((Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash -ne $hashes[0]) {
            throw 'Checksum mismatch. Please retry the installation.'
        }
        & $binary install
        if ($LASTEXITCODE -ne 0) { throw "Installation failed (exit $LASTEXITCODE)." }
        Write-Host 'Open a new terminal and run palepale.'
    } finally {
        [Net.ServicePointManager]::SecurityProtocol = $previousTls
        # Only remove the unique temporary directory created above.
        Remove-Item -LiteralPath $temp -Recurse -Force
    }
}

Install-Palepale
