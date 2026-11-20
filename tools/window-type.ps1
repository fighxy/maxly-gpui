# Post text (WM_CHAR) and optionally Enter to a window, without touching the real keyboard.
# Usage: powershell -File tools/window-type.ps1 -ProcessName maxrs -Text "Hi" -Enter
param(
    [Parameter(Mandatory = $true)][string]$ProcessName,
    [Parameter(Mandatory = $true)][string]$Text,
    [switch]$Enter
)
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class WinType {
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr hWnd, uint msg, IntPtr w, IntPtr l);
}
"@
$proc = Get-Process -Name $ProcessName -ErrorAction Stop | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
$hwnd = $proc.MainWindowHandle
foreach ($ch in $Text.ToCharArray()) {
    [void][WinType]::PostMessage($hwnd, 0x0102, [IntPtr][int]$ch, [IntPtr]1)  # WM_CHAR
    Start-Sleep -Milliseconds 30
}
if ($Enter) {
    [void][WinType]::PostMessage($hwnd, 0x0100, [IntPtr]0x0D, [IntPtr]1)  # WM_KEYDOWN VK_RETURN
    Start-Sleep -Milliseconds 30
    [void][WinType]::PostMessage($hwnd, 0x0101, [IntPtr]0x0D, [IntPtr]0xC0000001)  # WM_KEYUP
}
"typed $($Text.Length) chars"
