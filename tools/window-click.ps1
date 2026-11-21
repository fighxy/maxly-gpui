# Post a left click to a window at client coordinates, without moving the real cursor.
# Usage: powershell -File tools/window-click.ps1 -ProcessName maxly-gpui -X 289 -Y 25
param(
    [Parameter(Mandatory = $true)][string]$ProcessName,
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y
)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class WinClick {
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
}
"@
$proc = Get-Process -Name $ProcessName -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
$hwnd = $proc.MainWindowHandle
$l = [IntPtr](($Y -shl 16) -bor ($X -band 0xFFFF))
[void][WinClick]::PostMessage($hwnd, 0x0200, [IntPtr]0, $l)  # WM_MOUSEMOVE
Start-Sleep -Milliseconds 100
[void][WinClick]::PostMessage($hwnd, 0x0201, [IntPtr]1, $l)  # WM_LBUTTONDOWN
Start-Sleep -Milliseconds 80
[void][WinClick]::PostMessage($hwnd, 0x0202, [IntPtr]0, $l)  # WM_LBUTTONUP
"clicked $X,$Y"
