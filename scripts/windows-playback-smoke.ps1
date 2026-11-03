param(
    [string]$Package = (Join-Path $PSScriptRoot '..\dist\YTDugem-windows-x86_64.zip'),
    [string]$Reports = (Join-Path $PSScriptRoot '..\dist\playback-test-results'),
    [switch]$Online,
    [switch]$RealAudio
)
$ErrorActionPreference = 'Stop'
$work = Join-Path $env:TEMP ('BitChord portable playback test '+[Guid]::NewGuid().ToString('N'))
$saved = @{}
foreach ($key in @('PATH','BITCHORD_MPV','BITCHORD_YTDLP','BITCHORD_AUDIO_OUTPUT','BITCHORD_DATA_DIR')) {
    $saved[$key] = [Environment]::GetEnvironmentVariable($key,'Process')
}
try {
    New-Item -ItemType Directory -Force -Path $Reports | Out-Null
    $Reports = (Resolve-Path $Reports).ProviderPath
    Expand-Archive -LiteralPath (Resolve-Path $Package).Path -DestinationPath $work
    $exe = Join-Path $work 'YTDugem\YTDugem.exe'
    foreach ($tool in @('mpv','yt-dlp','ffmpeg','ffprobe','deno')) {
        if (!(Test-Path (Join-Path $work ('YTDugem\tools\'+$tool+'.exe')))) { throw ('Missing bundled '+$tool) }
    }
    # No externally installed audio tools, executable overrides, or helpful caller PATH.
    $env:PATH = Join-Path $env:SystemRoot 'System32'
    $env:BITCHORD_MPV = $null
    $env:BITCHORD_YTDLP = $null
    $env:BITCHORD_AUDIO_OUTPUT = if ($RealAudio) { $null } else { 'null' }
    $env:BITCHORD_DATA_DIR = Join-Path $work 'isolated library'
    $wav = Join-Path $work '440 Hz test tone.wav'
    $writer = New-Object IO.BinaryWriter ([IO.File]::Create($wav))
    try {
        $samples = 44100 * 5
        $writer.Write([Text.Encoding]::ASCII.GetBytes('RIFF')); $writer.Write([int](36+$samples*2))
        $writer.Write([Text.Encoding]::ASCII.GetBytes('WAVEfmt ')); $writer.Write([int]16)
        $writer.Write([int16]1); $writer.Write([int16]1); $writer.Write([int]44100); $writer.Write([int]88200)
        $writer.Write([int16]2); $writer.Write([int16]16)
        $writer.Write([Text.Encoding]::ASCII.GetBytes('data')); $writer.Write([int]($samples*2))
        for ($i=0; $i -lt $samples; $i++) { $writer.Write([int16](2000*[Math]::Sin(2*[Math]::PI*440*$i/44100))) }
    } finally { $writer.Dispose() }
    $sources = @{local=$wav}
    if ($Online) { $sources['youtube'] = 'https://www.youtube.com/watch?v=5NV6Rdv1a3I' }
    foreach ($name in $sources.Keys) {
        $report = Join-Path $work ($name+'.json')
        $stderr = Join-Path $work ($name+'.stderr.log')
        $arguments = '--check-playback "'+$sources[$name]+'" "'+$report+'"'
        $info = New-Object Diagnostics.ProcessStartInfo
        $info.FileName = $exe
        $info.Arguments = $arguments
        $info.UseShellExecute = $false
        $info.CreateNoWindow = $true
        $info.RedirectStandardError = $true
        $app = New-Object Diagnostics.Process
        $app.StartInfo = $info
        [void]$app.Start()
        $errors = $app.StandardError.ReadToEndAsync()
        if (!$app.WaitForExit(100000)) {
            & "$env:SystemRoot\System32\taskkill.exe" /PID $app.Id /T /F | Out-Null
            throw ('Playback test hung: '+$name)
        }
        $app.WaitForExit()
        [IO.File]::WriteAllText($stderr,$errors.GetAwaiter().GetResult())
        Copy-Item -LiteralPath $stderr -Destination (Join-Path $Reports ($name+'.stderr.log'))
        if (!(Test-Path $report)) { throw ('Playback test produced no report: '+$name+'; exit '+$app.ExitCode+'; '+(Get-Content -LiteralPath $stderr -Raw)) }
        Copy-Item -LiteralPath $report -Destination (Join-Path $Reports ($name+'.json'))
        $result = Get-Content -LiteralPath $report -Raw | ConvertFrom-Json
        if ($app.ExitCode -ne 0 -or !$result.ok) { throw ($name+': exit '+$app.ExitCode+'; '+$result.error) }
        if ($RealAudio -and $result.audio_backend -eq 'null') { throw 'Real audio was requested but null output was used' }
        if ($result.sample_rate -le 0 -or $result.position_after_resume -le 0) { throw 'No decoded audio advanced' }
        Write-Output ('PASS '+$name+': backend='+$result.audio_backend+'; sample_rate='+$result.sample_rate+'; checks='+($result.checks -join ','))
    }
    Write-Output ('PASS: extracted Windows package plays audio with system-only PATH; reports: '+$Reports)
} finally {
    foreach ($key in $saved.Keys) { [Environment]::SetEnvironmentVariable($key,$saved[$key],'Process') }
    if (Test-Path $work) { Remove-Item -LiteralPath $work -Recurse -Force }
}
