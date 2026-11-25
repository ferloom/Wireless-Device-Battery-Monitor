param([string]$OutputDirectory)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$OutputDirectory = if ($OutputDirectory) { $OutputDirectory } else { Join-Path $root 'target\xaml-preview\host' }
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw "Visual Studio locator not found: $vswhere" }
$vsInstall = & $vswhere -latest -products '*' -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $vsInstall) { throw 'Visual Studio 2022 C++ Build Tools are required.' }
$vs = Join-Path $vsInstall 'VC\Auxiliary\Build\vcvarsall.bat'
if (-not (Test-Path -LiteralPath $vs)) { throw "Visual Studio C++ environment script not found: $vs" }
$out = [IO.Path]::GetFullPath($OutputDirectory)
$pdb = Join-Path $root 'target\xaml-preview\artifacts\Taskbar.pdb'
if (-not (Test-Path $vs)) { throw "MSVC not found: $vs" }
if (-not (Test-Path $pdb)) { throw "Verified taskbar PDB missing: $pdb" }
New-Item -ItemType Directory -Force -Path $out | Out-Null
$cmd = Join-Path $out 'build-host.cmd'
$body = @"
@echo off
call "$vs" x64 >nul
if errorlevel 1 exit /b %errorlevel%
cd /d "$out"
if errorlevel 1 exit /b %errorlevel%
if not exist adapter mkdir adapter
if not exist host mkdir host
cl /nologo /LD /EHsc /std:c++20 /I"$root\src\experimental_xaml\vendor\publisher" /Foadapter\ "$root\src\experimental_xaml\vendor\taskbar-xaml-host-test\ExplorerTaskbarAdapter.cpp" "$root\src\experimental_xaml\vendor\taskbar-xaml-vm\TaskbarXamlModule.cpp" "$root\src\experimental_xaml\vendor\taskbar-xaml-widget\WidgetSurfaceView.cpp" /Fe:BatteryHubExplorerXamlAdapter.dll /link /IMPLIB:BatteryHubExplorerXamlAdapter.lib windowsapp.lib runtimeobject.lib shlwapi.lib shell32.lib advapi32.lib user32.lib
if errorlevel 1 exit /b %errorlevel%
cl /nologo /EHsc /std:c++20 /Fohost\ "$root\src\experimental_xaml\vendor\taskbar-xaml-host-test\TaskbarHostTest.cpp" /Fe:BatteryHubTaskbarHostTest.exe /link dbghelp.lib bcrypt.lib shell32.lib version.lib user32.lib advapi32.lib
if errorlevel 1 exit /b %errorlevel%
cl /nologo /EHsc /std:c++20 /I"$root\src\experimental_xaml\vendor\publisher" "$root\src\experimental_xaml\vendor\diagnostics\IndividualLayoutProtocolTest.cpp" /Fe:IndividualLayoutProtocolTest.exe
if errorlevel 1 exit /b %errorlevel%
IndividualLayoutProtocolTest.exe
if errorlevel 1 exit /b %errorlevel%
cl /nologo /EHsc /std:c++20 "$root\src\experimental_xaml\vendor\taskbar-xaml-host-test\HookMessageDeliveryTest.cpp" /Fe:HookMessageDeliveryTest.exe /link user32.lib
if errorlevel 1 exit /b %errorlevel%
HookMessageDeliveryTest.exe
if errorlevel 1 exit /b %errorlevel%
copy /y "$pdb" "$out\Taskbar.pdb" >nul
if errorlevel 1 exit /b %errorlevel%
exit /b 0
"@
Set-Content -LiteralPath $cmd -Value $body -Encoding Ascii
$proc = Start-Process -FilePath 'cmd.exe' -ArgumentList @('/d', '/c', "`"$cmd`"") -Wait -PassThru -NoNewWindow
if ($proc.ExitCode -ne 0) { throw "Native host build failed with exit $($proc.ExitCode)" }
Get-ChildItem -LiteralPath $out -File | Select-Object Name, Length
