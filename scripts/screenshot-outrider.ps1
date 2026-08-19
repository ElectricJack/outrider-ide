[void][System.Reflection.Assembly]::LoadWithPartialName('System.Drawing')

$signature = @'
[DllImport("user32.dll")]
public static extern IntPtr FindWindow(string lpClassName, string lpWindowName);

[DllImport("user32.dll")]
public static extern bool GetWindowRect(IntPtr hWnd, out RECT lpRect);

[DllImport("user32.dll")]
public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);

[DllImport("user32.dll")]
public static extern bool IsWindowVisible(IntPtr hWnd);

[DllImport("user32.dll", CharSet = CharSet.Unicode)]
public static extern int GetWindowText(IntPtr hWnd, System.Text.StringBuilder lpString, int nMaxCount);

public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

[DllImport("user32.dll")]
public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);

public struct RECT {
    public int Left, Top, Right, Bottom;
}
'@

try { $WinApi = Add-Type -MemberDefinition $signature -Name 'WinApi' -Namespace 'Screenshot' -PassThru -ErrorAction Stop }
catch { $WinApi = [Screenshot.WinApi] }

$outPath = $args[0]
if (-not $outPath) {
    $outPath = "$env:TEMP\outrider-screenshot.png"
}

# Find outrider window
$found = [IntPtr]::Zero
$callback = [Screenshot.WinApi+EnumWindowsProc]{
    param([IntPtr]$hWnd, [IntPtr]$lParam)
    if (-not [Screenshot.WinApi]::IsWindowVisible($hWnd)) { return $true }
    $sb = New-Object System.Text.StringBuilder 256
    [void][Screenshot.WinApi]::GetWindowText($hWnd, $sb, 256)
    $title = $sb.ToString()
    if ($title -match '^outrider \u2014 ') {
        $script:found = $hWnd
        return $false
    }
    return $true
}
[void][Screenshot.WinApi]::EnumWindows($callback, [IntPtr]::Zero)

if ($found -eq [IntPtr]::Zero) {
    Write-Error "No outrider window found"
    exit 1
}

# Get window rect
$rect = New-Object Screenshot.WinApi+RECT
[void][Screenshot.WinApi]::GetWindowRect($found, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top

if ($w -le 0 -or $h -le 0) {
    Write-Error "Window has zero size"
    exit 1
}

# Capture using PrintWindow
$bmp = New-Object System.Drawing.Bitmap $w, $h
$gfx = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $gfx.GetHdc()
[void][Screenshot.WinApi]::PrintWindow($found, $hdc, 2)
$gfx.ReleaseHdc($hdc)
$gfx.Dispose()

$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

Write-Output $outPath
