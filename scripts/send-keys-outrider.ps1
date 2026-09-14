# Foreground the outrider window and send keystrokes (SendKeys syntax),
# e.g.  send-keys-outrider.ps1 '{HOME}'   or   send-keys-outrider.ps1 '{ENTER}{END}'
# Optional second arg: milliseconds to wait after sending (default 1500).
param(
    [Parameter(Mandatory = $true)][string]$Keys,
    [int]$SettleMs = 1500
)
Add-Type -AssemblyName System.Windows.Forms

$sig = @'
[DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
[DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder s, int n);
public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
[DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc f, IntPtr p);
'@
try { $null = Add-Type -MemberDefinition $sig -Name 'KeyApi' -Namespace 'OutriderKeys' -PassThru -ErrorAction Stop }
catch {}

$script:found = [IntPtr]::Zero
$cb = [OutriderKeys.KeyApi+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    if (-not [OutriderKeys.KeyApi]::IsWindowVisible($hWnd)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][OutriderKeys.KeyApi]::GetWindowText($hWnd, $sb, 256)
    if ($sb.ToString() -match '^outrider \u2014 ') { $script:found = $hWnd; return $false }
    return $true
}
[void][OutriderKeys.KeyApi]::EnumWindows($cb, [IntPtr]::Zero)
if ($script:found -eq [IntPtr]::Zero) { Write-Error 'No outrider window found'; exit 1 }

[void][OutriderKeys.KeyApi]::SetForegroundWindow($script:found)
Start-Sleep -Milliseconds 400
[System.Windows.Forms.SendKeys]::SendWait($Keys)
Start-Sleep -Milliseconds $SettleMs
Write-Output "sent: $Keys"
