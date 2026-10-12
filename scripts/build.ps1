# build.ps1
# Module: Palepale Windows build and verification
# Build: PowerShell 7 / Rust 1.98.1 / MSVC / CMake / NASM

param(
    [ValidateSet('build', 'test', 'lint', 'e2e')]
    [string]$Task = 'build',
    [string]$TargetDirectory = (Join-Path $PSScriptRoot '../target')
)

$ErrorActionPreference = 'Stop'
$repoRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$env:CARGO_TARGET_DIR = [System.IO.Path]::GetFullPath($TargetDirectory)
if ($IsWindows) {
    $architecture = [System.Runtime.InteropServices.RuntimeInformation]::ProcessArchitecture
    $env:PROCESSOR_ARCHITECTURE = switch ($architecture) {
        'X64' { 'AMD64' }
        'X86' { 'x86' }
        'Arm64' { 'ARM64' }
        default { throw "Unsupported architecture: $architecture" }
    }
    if (-not $env:COMPUTERNAME) { $env:COMPUTERNAME = [System.Environment]::MachineName }
    $clangFolder = Join-Path $env:ProgramFiles 'LLVM/bin'
    if (-not $env:LIBCLANG_PATH -and (Test-Path (Join-Path $clangFolder 'libclang.dll'))) {
        $env:LIBCLANG_PATH = $clangFolder
    }
}

Push-Location $repoRoot
try {
    if ($Task -eq 'lint') {
        & cargo +1.98.1 fmt -- --check
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
        & cargo +1.98.1 clippy --locked --all-targets -- -D warnings
    } else {
        $cargoArgs = switch ($Task) {
            'build' { @('build', '--locked', '--release') }
            'test' { @('test', '--locked', '--bins') }
            'e2e' { @('test', '--locked', '--test', 'e2e') }
        }
        & cargo +1.98.1 @cargoArgs
    }
    exit $LASTEXITCODE
} finally {
    Pop-Location
}
