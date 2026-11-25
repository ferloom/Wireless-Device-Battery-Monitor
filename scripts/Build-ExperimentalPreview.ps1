$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Push-Location $root
try {
    cargo +1.99.0-x86_64-pc-windows-msvc build --release --features experimental-xaml
    if ($LASTEXITCODE -ne 0) { throw "Cargo build failed: $LASTEXITCODE" }
    $candidate = Join-Path $root 'target\xaml-preview\candidate'
    $run = Join-Path $candidate 'run'
    $hostDir = Join-Path $candidate 'host'
    New-Item -ItemType Directory -Force -Path $run | Out-Null
    Copy-Item -LiteralPath (Join-Path $root 'target\release\batteryhub-taskbar.exe') `
        -Destination (Join-Path $run 'BatteryHub-Experimental.exe') -Force
    Copy-Item -LiteralPath (Join-Path $root 'target\release\batteryhub-experimental-supervisor.exe') `
        -Destination (Join-Path $run 'BatteryHub-Experimental-Supervisor.exe') -Force
    & (Join-Path $PSScriptRoot 'Build-ExperimentalXaml.ps1') -OutputDirectory $hostDir
    if ($LASTEXITCODE -ne 0) { throw "Native bridge build failed: $LASTEXITCODE" }
} finally {
    Pop-Location
}
