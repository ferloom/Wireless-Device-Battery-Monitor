$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$candidate = Join-Path $root 'target\xaml-preview\candidate'
$run = Join-Path $candidate 'run'
$appPath = Join-Path $run 'BatteryHub-Experimental.exe'
$supervisorPath = Join-Path $run 'BatteryHub-Experimental-Supervisor.exe'
$hostPath = Join-Path $candidate 'host\BatteryHubTaskbarHostTest.exe'

if (-not (Test-Path -LiteralPath $supervisorPath)) { throw 'Experimental supervisor is missing; refusing an uncoordinated shutdown.' }
$supervisors = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHub-Experimental-Supervisor.exe'" | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath.Equals($supervisorPath, [StringComparison]::OrdinalIgnoreCase)
})
$apps = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHub-Experimental.exe'" | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath.Equals($appPath, [StringComparison]::OrdinalIgnoreCase)
})
if ($supervisors.Count -gt 1 -or $apps.Count -gt 1) { throw 'Multiple experimental instances were found; no process was closed.' }
if (-not $supervisors.Count -and -not $apps.Count) { Write-Host 'No experimental BatteryHub instance is running.'; return }

& $supervisorPath --stop
if ($LASTEXITCODE -ne 0) { throw "The supervisor stop request failed: $LASTEXITCODE" }
$deadline = (Get-Date).AddSeconds(20)
do {
    $apps = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHub-Experimental.exe'" | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.Equals($appPath, [StringComparison]::OrdinalIgnoreCase)
    })
    $supervisors = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHub-Experimental-Supervisor.exe'" | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.Equals($supervisorPath, [StringComparison]::OrdinalIgnoreCase)
    })
    $hosts = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHubTaskbarHostTest.exe'" | Where-Object {
        $_.ExecutablePath -and $_.ExecutablePath.Equals($hostPath, [StringComparison]::OrdinalIgnoreCase)
    })
    if (-not $apps.Count -and -not $supervisors.Count -and -not $hosts.Count) { break }
    Start-Sleep -Milliseconds 250
} while ((Get-Date) -lt $deadline)
if ($apps.Count -or $supervisors.Count -or $hosts.Count) { throw 'Experimental BatteryHub did not fully close; no process was force-terminated.' }
Write-Host 'Experimental BatteryHub closed cleanly. A stable instance is restored only if the supervisor paused one before startup.'
