$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$previewRoot = Join-Path $root 'target\xaml-preview'
$candidate = Join-Path $previewRoot 'candidate'
$run = Join-Path $candidate 'run'
$hostDir = Join-Path $candidate 'host'
$appPath = Join-Path $run 'BatteryHub-Experimental.exe'
$supervisorPath = Join-Path $run 'BatteryHub-Experimental-Supervisor.exe'
$hostPath = Join-Path $hostDir 'BatteryHubTaskbarHostTest.exe'
$hostLog = Join-Path $hostDir 'BatteryHubExperimentalXamlService.log'
$supervisorLog = Join-Path $candidate 'profile\logs\supervisor.log'

foreach ($path in $appPath, $supervisorPath, $hostPath) {
    if (-not (Test-Path -LiteralPath $path)) { throw "Required experimental file is missing: $path" }
}
$busy = @(Get-CimInstance Win32_Process | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath -match '\\BatteryHub-(Experimental|Experimental-Supervisor)\.exe$'
})
if ($busy.Count) { throw 'An experimental BatteryHub process is already running; close it before starting another session.' }

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $supervisorLog) | Out-Null
Remove-Item -LiteralPath $hostLog, $supervisorLog -Force -ErrorAction SilentlyContinue
$supervisor = Start-Process -FilePath $supervisorPath -WorkingDirectory $run -WindowStyle Hidden -PassThru
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline -and -not $supervisor.HasExited) {
    if ((Get-Content -LiteralPath $hostLog -Raw -ErrorAction SilentlyContinue) -match 'event=service-attached') {
        Write-Host 'Experimental BatteryHub is running with live battery polling and native taskbar integration.'
        Write-Host 'Use scripts\Stop-ExperimentalPreview.ps1 to close it; the isolated supervisor restores a stable instance if it paused one.'
        Write-Host "XAML service log: $hostLog"
        return
    }
    Start-Sleep -Milliseconds 500
}
$appRunning = @(Get-CimInstance Win32_Process -Filter "Name='BatteryHub-Experimental.exe'" | Where-Object {
    $_.ExecutablePath -and $_.ExecutablePath.Equals($appPath, [StringComparison]::OrdinalIgnoreCase)
}).Count -gt 0
if ($supervisor.HasExited -and -not $appRunning) {
    $detail = Get-Content -LiteralPath $supervisorLog -Raw -ErrorAction SilentlyContinue
    throw "The experimental supervisor did not start the candidate. $detail"
}
Write-Warning "The candidate is running with BatteryHub's traditional fallback; the XAML host did not confirm attachment. The supervisor remains active and will restore any previously running stable instance after exit."
