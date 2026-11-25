if (-not ('BatteryHubExperimentalNative' -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class BatteryHubExperimentalNative {
  public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr parameter);
  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr parameter);
  [DllImport("user32.dll", CharSet=CharSet.Unicode, SetLastError=true)]
  public static extern int GetClassName(IntPtr hwnd, System.Text.StringBuilder className, int maxCount);
  [DllImport("user32.dll", SetLastError=true)]
  public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
  [DllImport("user32.dll", SetLastError=true)]
  public static extern bool PostMessage(IntPtr hwnd, uint message, IntPtr wParam, IntPtr lParam);
}
'@
}

function Send-BatteryHubCloseMessage([int]$ProcessId, [string]$WindowClass) {
    $script:BatteryHubTargetWindow = [IntPtr]::Zero
    $callback = [BatteryHubExperimentalNative+EnumWindowsProc]{
        param($hwnd, $parameter)
        $name = [System.Text.StringBuilder]::new(128)
        [void][BatteryHubExperimentalNative]::GetClassName($hwnd, $name, $name.Capacity)
        $ownerPid = [uint32]0
        [void][BatteryHubExperimentalNative]::GetWindowThreadProcessId($hwnd, [ref]$ownerPid)
        if ($ownerPid -eq $ProcessId -and $name.ToString() -eq $WindowClass) {
            $script:BatteryHubTargetWindow = $hwnd
            return $false
        }
        return $true
    }
    [void][BatteryHubExperimentalNative]::EnumWindows($callback, [IntPtr]::Zero)
    if ($script:BatteryHubTargetWindow -eq [IntPtr]::Zero) { return $false }
    return [BatteryHubExperimentalNative]::PostMessage($script:BatteryHubTargetWindow, 0x0010, [IntPtr]::Zero, [IntPtr]::Zero)
}
