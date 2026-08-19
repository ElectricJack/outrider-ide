# Click at window-relative client coordinates (x, y) inside the outrider window.
# Coordinates match screenshot pixels from screenshot-outrider.ps1 (which
# captures the full window incl. title bar), so pass screenshot coords directly.
param(
    [Parameter(Mandatory = $true)][int]$X,
    [Parameter(Mandatory = $true)][int]$Y,
    [int]$SettleMs = 800
)
$sig = @'
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
[DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
[DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder s, int n);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc f, IntPtr p);
public struct RECT { public int Left, Top, Right, Bottom; }
'@
try { $null = Add-Type -MemberDefinition $sig -Name 'ClickApi' -Namespace 'OutriderClick' -PassThru -ErrorAction Stop } catch {}

$script:found = [IntPtr]::Zero
$cb = [OutriderClick.ClickApi+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    if (-not [OutriderClick.ClickApi]::IsWindowVisible($hWnd)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][OutriderClick.ClickApi]::GetWindowText($hWnd, $sb, 256)
    if ($sb.ToString() -match '^outrider \u2014 ') { $script:found = $hWnd; return $false }
    return $true
}
[void][OutriderClick.ClickApi]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:found -eq [IntPtr]::Zero) { Write-Error 'No outrider window found'; exit 1 }

[void][OutriderClick.ClickApi]::SetForegroundWindow($script:found)
Start-Sleep -Milliseconds 300
$r = New-Object OutriderClick.ClickApi+RECT
[void][OutriderClick.ClickApi]::GetWindowRect($script:found, [ref]$r)
$sx = $r.Left + $X
$sy = $r.Top + $Y
[void][OutriderClick.ClickApi]::SetCursorPos($sx, $sy)
Start-Sleep -Milliseconds 120
# MOUSEEVENTF_LEFTDOWN = 2, MOUSEEVENTF_LEFTUP = 4
[OutriderClick.ClickApi]::mouse_event(2, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds 60
[OutriderClick.ClickApi]::mouse_event(4, 0, 0, 0, [UIntPtr]::Zero)
Start-Sleep -Milliseconds $SettleMs
Write-Output "clicked ($X,$Y) -> screen ($sx,$sy)"
