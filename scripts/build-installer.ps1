<#
.SYNOPSIS
Builds dist/YTDugem-windows-x86_64-setup.exe from dist/YTDugem-windows-x86_64.zip.

Run scripts/package-windows.py first. Needs Inno Setup 6 or later (ISCC.exe on PATH, in its
default folder, or given with -Iscc). -Test installs the result silently into a temporary folder, checks
the files, starts the app, and uninstalls it again.
#>
param(
    [string]$Iscc,
    [switch]$Test
)
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$zip = Join-Path $root 'dist\YTDugem-windows-x86_64.zip'
if (-not (Test-Path $zip)) { throw "Missing $zip; run scripts/package-windows.py first" }

if (-not $Iscc) {
    $found = Get-Command ISCC.exe -ErrorAction SilentlyContinue
    $installed = "${env:ProgramFiles(x86)}", $env:ProgramFiles, "$env:LOCALAPPDATA\Programs" |
        Where-Object { $_ } |
        ForEach-Object { Get-ChildItem -Path (Join-Path $_ 'Inno Setup *\ISCC.exe') -ErrorAction SilentlyContinue } |
        Sort-Object FullName -Descending | Select-Object -ExpandProperty FullName
    $candidates = @($(if ($found) { $found.Source })) + @($installed) | Where-Object { $_ }
    if (-not $candidates) { throw 'Inno Setup not found; install it or pass -Iscc' }
    $Iscc = @($candidates)[0]
}

$metadata = cargo metadata --no-deps --format-version 1 --manifest-path (Join-Path $root 'Cargo.toml') | ConvertFrom-Json
$version = ($metadata.packages | Where-Object name -eq 'ytdugem').version

$staging = Join-Path $root 'target\installer'
if (Test-Path $staging) { Remove-Item -Recurse -Force $staging }
New-Item -ItemType Directory -Force $staging | Out-Null
& "$env:SystemRoot\System32\tar.exe" -xf $zip -C $staging
if ($LASTEXITCODE) { throw "Could not extract $zip" }
$source = Join-Path $staging 'YTDugem'

& $Iscc /Q "/DAppVersion=$version" "/DSourceDir=$source" "/DOutputDir=$(Join-Path $root 'dist')" (Join-Path $root 'packaging\windows\ytdugem.iss')
if ($LASTEXITCODE) { throw "ISCC failed with $LASTEXITCODE" }
$setup = Join-Path $root 'dist\YTDugem-windows-x86_64-setup.exe'
Write-Host "Installer: $setup"

if ($Test) {
    $dir = Join-Path $env:TEMP "ytdugem-installer-test-$PID"
    $data = Join-Path $env:TEMP "ytdugem-installer-data-$PID"
    $log = Join-Path $env:TEMP "ytdugem-installer-$PID.log"
    $p = Start-Process $setup -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/DIR=$dir", "/LOG=$log" -Wait -PassThru
    if ($p.ExitCode) { throw "Setup exited with $($p.ExitCode); see $log" }
    try {
        foreach ($file in 'YTDugem.exe', 'tools\mpv.exe', 'tools\yt-dlp.exe', 'tools\ffmpeg.exe', 'tools\deno.exe', 'unins000.exe') {
            if (-not (Test-Path (Join-Path $dir $file))) { throw "Installed copy is missing $file" }
        }
        $env:BITCHORD_DATA_DIR = $data
        $env:YTDUGEM_NO_UPDATE_CHECK = '1'
        $app = Start-Process (Join-Path $dir 'YTDugem.exe') -PassThru
        Start-Sleep -Seconds 5
        if ($app.HasExited) { throw "Installed YTDugem exited at start with $($app.ExitCode)" }
        Stop-Process $app -Force
        $app.WaitForExit()
        Write-Host 'Installed copy starts.'
    } finally {
        $u = Start-Process (Join-Path $dir 'unins000.exe') -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
        # The uninstaller hands off to a temporary copy; give it a moment to finish.
        for ($i = 0; $i -lt 30 -and (Test-Path (Join-Path $dir 'YTDugem.exe')); $i++) { Start-Sleep -Milliseconds 500 }
        Remove-Item -Recurse -Force $data -ErrorAction SilentlyContinue
    }
    if (Test-Path (Join-Path $dir 'YTDugem.exe')) { throw "Uninstall left files in $dir" }
    if (Test-Path (Join-Path $dir 'tools')) { throw "Uninstall left $dir\tools" }
    Remove-Item -Recurse -Force $dir -ErrorAction SilentlyContinue
    Write-Host 'Install, start and uninstall passed.'
}
