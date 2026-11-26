param([string]$Compiler = 'g++', [string]$Cargo = 'cargo')
$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    $output = 'dist\MyFinder'
    New-Item -ItemType Directory -Force $output | Out-Null
    & $Compiler -std=c++17 -DUNICODE -D_UNICODE -O2 -shared -static native/myfinder.cpp -o "$output\BatteryHubMyFinder.dll" -ld3d11 -ldxgi -ld2d1 -ldcomp -ldwmapi -lgdi32
    if ($LASTEXITCODE) { throw 'MyFinder DLL build failed' }
    & $Cargo build --locked --release --bin batteryhub-taskbar
    if ($LASTEXITCODE) { throw 'BatteryHub build failed' }
    Copy-Item -LiteralPath 'target\release\batteryhub-taskbar.exe' -Destination "$output\batteryhub-taskbar.exe" -Force
    Copy-Item -LiteralPath 'native\Start-MyFinder.cmd' -Destination "$output\Start-MyFinder.cmd" -Force
} finally {
    Pop-Location
}
