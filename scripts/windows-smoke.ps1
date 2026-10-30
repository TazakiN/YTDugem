param([string]$Executable = (Join-Path $PSScriptRoot '..\dist\YTDugem.exe'))
$ErrorActionPreference = 'Stop'
Add-Type @'
using System;
using System.Runtime.InteropServices;
public class BitChordSmokeWindow {
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool PostMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
'@
$previousData = $env:BITCHORD_DATA_DIR
$testData = Join-Path $env:TEMP ('bitchord-native-smoke-'+[Guid]::NewGuid().ToString('N'))
$env:BITCHORD_DATA_DIR = $testData
$app = $null
try {
    $app = Start-Process -FilePath (Resolve-Path $Executable).Path -PassThru
    $until = [DateTime]::UtcNow.AddSeconds(30)
    do {
        $app.Refresh()
        if ($app.MainWindowTitle -eq 'YTDugem') { break }
        if ($app.WaitForExit(100)) { throw ('App exited before showing a window: '+$app.ExitCode) }
    } while ([DateTime]::UtcNow -lt $until)
    if ($app.MainWindowTitle -ne 'YTDugem') { throw 'No YTDugem window appeared' }
    if (![BitChordSmokeWindow]::IsWindowVisible($app.MainWindowHandle)) { throw 'Window is not visible' }
    Write-Output ('Visible native Windows window: '+$app.MainWindowTitle+'; PID: '+$app.Id)
    [void][BitChordSmokeWindow]::PostMessage($app.MainWindowHandle,0x0010,[IntPtr]::Zero,[IntPtr]::Zero)
    if (!$app.WaitForExit(10000)) { throw 'App did not close normally' }
    if ($app.ExitCode -ne 0) { throw ('App exit code: '+$app.ExitCode) }
    Write-Output 'PASS: native Windows startup, visible window, clean shutdown'
} finally {
    if ($app -and !$app.HasExited) { $app.Kill() }
    $env:BITCHORD_DATA_DIR = $previousData
    if (Test-Path $testData) { Remove-Item -LiteralPath $testData -Recurse -Force }
}
