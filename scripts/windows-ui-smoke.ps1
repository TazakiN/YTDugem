param(
    [string]$Executable = (Join-Path $PSScriptRoot '..\dist\YTDugem.exe'),
    [int]$TimeoutSeconds = 30
)
$ErrorActionPreference = 'Stop'
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) { throw 'This smoke test requires Windows.' }

Add-Type -AssemblyName System.Drawing
Add-Type -AssemblyName System.Windows.Forms
Add-Type @'
using System;
using System.Runtime.InteropServices;
public static class UiSmokeNative {
    [StructLayout(LayoutKind.Sequential)] public struct INPUT { public uint type; public InputUnion U; }
    [StructLayout(LayoutKind.Explicit)] public struct InputUnion { [FieldOffset(0)] public KEYBDINPUT ki; [FieldOffset(0)] public MOUSEINPUT mi; }
    [StructLayout(LayoutKind.Sequential)] public struct KEYBDINPUT { public ushort wVk, wScan; public uint dwFlags, time; public IntPtr dwExtraInfo; }
    [StructLayout(LayoutKind.Sequential)] public struct MOUSEINPUT { public int dx, dy; public uint mouseData, dwFlags, time; public IntPtr dwExtraInfo; }
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
    [DllImport("user32.dll", SetLastError=true)] public static extern uint SendInput(uint n, INPUT[] inputs, int size);
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int Left, Top, Right, Bottom; }
    public static void Key(ushort vk, bool up) { INPUT i = new INPUT(); i.type=1; i.U.ki.wVk=vk; i.U.ki.dwFlags=up?2u:0u; if(SendInput(1,new INPUT[]{i},Marshal.SizeOf(typeof(INPUT)))!=1) throw new System.ComponentModel.Win32Exception(); }
    public static void Char(char c, bool up) { INPUT i = new INPUT(); i.type=1; i.U.ki.wScan=c; i.U.ki.dwFlags=4u|(up?2u:0u); if(SendInput(1,new INPUT[]{i},Marshal.SizeOf(typeof(INPUT)))!=1) throw new System.ComponentModel.Win32Exception(); }
}
'@
[void][UiSmokeNative]::SetProcessDPIAware()

$exe = (Resolve-Path -LiteralPath $Executable).Path
$root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$tools = Join-Path (Split-Path $exe) 'tools'
if (-not (Test-Path -LiteralPath $tools -PathType Container)) { throw "Bundled tools folder missing beside executable: $tools" }
$out = Join-Path $root 'dist\ui-test-results'
New-Item -ItemType Directory -Force -Path $out | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'
$reportPath = Join-Path $out "windows-ui-$stamp.json"
$data = Join-Path $env:TEMP ('ytdugem-ui-smoke-' + [guid]::NewGuid().ToString('N'))
$audio = Join-Path $data 'ui-smoke.wav'
$libraryFile = Join-Path $data 'library.json'
$previousData = $env:BITCHORD_DATA_DIR
$previousAudio = $env:BITCHORD_AUDIO_OUTPUT
$app = $null
$hwnd = [IntPtr]::Zero
$checks = [System.Collections.Generic.List[object]]::new()
$report = [ordered]@{ ok = $false; executable = $exe; data_directory = $data; audio_backend = 'null'; checks = $checks; limitations = @('Null output checks decoding/playback state, not speaker output.', 'Lyrics/queue tab selection has screenshot evidence only; no public persisted tab-state signal.', 'Search focus is checked by typed text visible in the captured app window; no live query is submitted.') }

function Assert-AppForeground {
    if ($app.HasExited -or [UiSmokeNative]::GetForegroundWindow() -ne $hwnd) { throw 'Refusing keyboard input: YTDugem is not the foreground window.' }
}
function Focus-App {
    [void][UiSmokeNative]::SetForegroundWindow($hwnd)
    $until = [DateTime]::UtcNow.AddSeconds(5)
    while ([DateTime]::UtcNow -lt $until) {
        if ([UiSmokeNative]::GetForegroundWindow() -eq $hwnd) { return }
        Start-Sleep -Milliseconds 100
    }
    throw 'Could not focus YTDugem; refusing to send keyboard input.'
}
function Send-Key([int]$vk) {
    Assert-AppForeground
    [UiSmokeNative]::Key([uint16]$vk, $false); [UiSmokeNative]::Key([uint16]$vk, $true)
}
function Send-Combo([int[]]$modifiers, [int]$vk) {
    Assert-AppForeground
    foreach ($m in $modifiers) { [UiSmokeNative]::Key([uint16]$m, $false) }
    [UiSmokeNative]::Key([uint16]$vk, $false); [UiSmokeNative]::Key([uint16]$vk, $true)
    for ($i = $modifiers.Length - 1; $i -ge 0; $i--) { [UiSmokeNative]::Key([uint16]$modifiers[$i], $true) }
}
function Send-Text([string]$text) {
    Assert-AppForeground
    foreach ($c in $text.ToCharArray()) { [UiSmokeNative]::Char($c, $false); [UiSmokeNative]::Char($c, $true) }
}
function Save-Screenshot([string]$name) {
    $rect = New-Object UiSmokeNative+RECT
    if (-not [UiSmokeNative]::GetWindowRect($hwnd, [ref]$rect)) { throw 'Cannot get app window bounds for screenshot.' }
    $width = $rect.Right - $rect.Left; $height = $rect.Bottom - $rect.Top
    if ($width -lt 100 -or $height -lt 100) { throw "Invalid app window size ${width}x${height}." }
    $bitmap = [System.Drawing.Bitmap]::new($width, $height)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try { $graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size) } finally { $graphics.Dispose() }
        $path = Join-Path $out "$stamp-$name.png"
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
        $checks.Add([ordered]@{ assertion = "Captured $name app-window screenshot"; evidence = $path; window_size = "${width}x${height}" })
    } finally { $bitmap.Dispose() }
}
function Wait-Until([scriptblock]$condition, [string]$failure) {
    $until = [DateTime]::UtcNow.AddSeconds($TimeoutSeconds)
    while ([DateTime]::UtcNow -lt $until) {
        $app.Refresh()
        if ($app.HasExited) { throw "App exited early (code $($app.ExitCode)): $failure" }
        if (& $condition) { return }
        Start-Sleep -Milliseconds 200
    }
    throw $failure
}
function Read-Library {
    if (-not (Test-Path -LiteralPath $libraryFile)) { return $null }
    try { return Get-Content -LiteralPath $libraryFile -Raw | ConvertFrom-Json } catch { return $null }
}
function Normalize-Path([string]$path) {
    if ($path.StartsWith('\\?\', [StringComparison]::Ordinal)) { $path = $path.Substring(4) }
    return [IO.Path]::GetFullPath($path).TrimEnd('\')
}

try {
    New-Item -ItemType Directory -Path $data | Out-Null
    # 1-second mono PCM WAV fixture: header plus silence, no external encoder needed.
    $stream = [IO.File]::Create($audio)
    try {
        $writer = [IO.BinaryWriter]::new($stream)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([int]16036); $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVE'))
        $writer.Write([Text.Encoding]::ASCII.GetBytes('fmt ')); $writer.Write([int]16); $writer.Write([int16]1); $writer.Write([int16]1)
        $writer.Write([int]8000); $writer.Write([int]16000); $writer.Write([int16]2); $writer.Write([int16]16)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([int]16000); $writer.Write([byte[]]::new(16000))
    } finally { $stream.Dispose() }
    $env:BITCHORD_DATA_DIR = $data
    $env:BITCHORD_AUDIO_OUTPUT = 'null'
    $app = Start-Process -FilePath $exe -WorkingDirectory (Split-Path $exe) -PassThru
    Wait-Until { $app.MainWindowTitle -eq 'YTDugem' -and $app.MainWindowHandle -ne [IntPtr]::Zero } 'No YTDugem window appeared.'
    $hwnd = $app.MainWindowHandle
    if (-not [UiSmokeNative]::IsWindowVisible($hwnd)) { throw 'YTDugem window is not visible.' }
    $workArea = [System.Windows.Forms.Screen]::FromHandle($hwnd).WorkingArea
    [void][UiSmokeNative]::SetWindowPos($hwnd, [IntPtr]::Zero, $workArea.Left, $workArea.Top, [Math]::Min(1280, $workArea.Width), [Math]::Min(800, $workArea.Height), 0x0004)
    Focus-App
    Save-Screenshot '01-home'
    $checks.Add([ordered]@{ assertion = 'Visible YTDugem native window'; pid = $app.Id; hwnd = $hwnd.ToInt64() })

    Send-Combo @(0x11) 0x4c # Ctrl+L
    Start-Sleep -Milliseconds 350
    Send-Text $audio
    Send-Key 0x0d
    Wait-Until { $lib = Read-Library; $lib -and @($lib.local).Count -eq 1 } 'Imported track was not persisted in library.json.'
    $lib = Read-Library
    if ((Normalize-Path ([string]$lib.local[0].source)) -ne (Normalize-Path $audio)) { throw "Persisted source did not match fixture: $($lib.local[0].source)" }
    $checks.Add([ordered]@{ assertion = 'Ctrl+L path import persisted one local track'; title = $lib.local[0].title; source = $lib.local[0].source })
    Save-Screenshot '02-local-import'

    Send-Combo @(0x11) 0x0d # Ctrl+Enter
    Wait-Until { $lib = Read-Library; $lib -and @($lib.history).Count -ge 1 } 'Ctrl+Enter did not persist local playback history.'
    $lib = Read-Library
    if ((Normalize-Path ([string]$lib.history[0].source)) -ne (Normalize-Path $audio)) { throw 'Playback history does not identify the local fixture.' }
    if (@($lib.queue).Count -ne 0) { throw "Single-track playback should leave an empty queue; found $(@($lib.queue).Count) queued tracks." }
    $checks.Add([ordered]@{ assertion = 'Ctrl+Enter persisted fixture in history; single-track queue is empty as expected'; history_count = @($lib.history).Count; queue_count = @($lib.queue).Count })
    Save-Screenshot '03-local-playback'

    Send-Combo @(0x11) 0x46 # Ctrl+F
    Start-Sleep -Milliseconds 300
    Send-Text 'UI smoke search focus'
    Save-Screenshot '04-search-focus'
    $checks.Add([ordered]@{ assertion = 'Ctrl+F received text; screenshot shows whether the search field had focus'; network_submitted = $false })

    Send-Key 0x7a # F11
    Start-Sleep -Milliseconds 300
    Save-Screenshot '05-expanded-player'
    Send-Key 0x4c # L
    Start-Sleep -Milliseconds 250
    Save-Screenshot '06-expanded-lyrics'
    Send-Key 0x51 # Q
    Start-Sleep -Milliseconds 250
    Save-Screenshot '07-expanded-queue'
    Send-Key 0x1b # Escape
    Start-Sleep -Milliseconds 250
    Save-Screenshot '08-after-escape'
    $checks.Add([ordered]@{ assertion = 'F11, L, Q, Escape shortcut sequence completed'; tabs = 'Screenshots are visual evidence only, not an asserted semantic tab-state.' })

    [void][UiSmokeNative]::SetWindowPos($hwnd, [IntPtr]::Zero, 80, 80, 1280, 800, 0x0004)
    Start-Sleep -Milliseconds 350
    Save-Screenshot '09-normal-size'
    [void][UiSmokeNative]::SetWindowPos($hwnd, [IntPtr]::Zero, 80, 80, 760, 560, 0x0004)
    Start-Sleep -Milliseconds 350
    if ($app.HasExited) { throw 'App exited after window resize.' }
    Save-Screenshot '10-narrow-size'
    $checks.Add([ordered]@{ assertion = 'App survived 1280x800 and 760x560 window sizes'; clean_shutdown = 'Pending' })

    Focus-App
    Send-Combo @(0x11) 0x51 # Ctrl+Q, app-target foreground checked
    if (-not $app.WaitForExit(10000)) { throw 'YTDugem did not shut down within 10 seconds.' }
    if ($app.ExitCode -ne 0) { throw "YTDugem exited with code $($app.ExitCode)." }
    $checks[$checks.Count - 1].clean_shutdown = 'Exit code 0 after Ctrl+Q'
    $report.ok = $true
    Write-Output 'PASS: Windows UI import, persisted local history, keyboard sequence, resize survival, clean shutdown (null audio output).'
    Write-Output "Report: $reportPath"
    Write-Output "Screenshots: $out"
} finally {
    if ($app -and -not $app.HasExited) { try { $app.Kill(); $app.WaitForExit(5000) } catch {} }
    $env:BITCHORD_DATA_DIR = $previousData
    $env:BITCHORD_AUDIO_OUTPUT = $previousAudio
    $report | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $reportPath -Encoding UTF8
    if (Test-Path -LiteralPath $data) { Remove-Item -LiteralPath $data -Recurse -Force }
}
