# Model-checks the in-app updater's folder swap (verification/update-swap) with TLC.
# Usage: powershell -NoProfile -File scripts/verify-update-swap.ps1
# Exit codes: 0 every model matched its expectation, 1 a model did not, 2 setup failed,
# 3 models matched but the covered code changed since the skill's sync commit.
$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
$spec = Join-Path $root 'verification\update-swap'
$work = Join-Path $root 'target\tla'
$jar = Join-Path $work 'tla2tools.jar'
$jarUrl = 'https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar'
$jarSha = '936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88'
$skill = Join-Path $root '.claude\skills\verify-update-swap\SKILL.md'
$covered = @('src/update.rs', 'src/ui/updater.rs')

# Each model and what it must produce: 'pass', or the invariant TLC must report violated.
$expected = [ordered]@{
    'real-rollback-ok'       = 'pass'                      # the code as written
    'real-rollback-fails'    = 'pass'                      # rollback renames may fail: nothing lost, report honest
    'broken-no-inline-undo'  = 'FailureLeavesAppUnchanged' # proves the check can fail
    'broken-no-rollback-old' = 'FailureLeavesAppUnchanged' # proves the check can fail
}

if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    Write-Host 'Java 11 or newer is required to run TLC.'; exit 2
}
New-Item -ItemType Directory -Force (Join-Path $work 'out') | Out-Null
if (-not (Test-Path $jar)) {
    Write-Host "Downloading TLA+ tools v1.7.4..."
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -UseBasicParsing -Uri $jarUrl -OutFile "$jar.part"
    Move-Item -Force "$jar.part" $jar
}
$actualSha = (Get-FileHash -Algorithm SHA256 $jar).Hash.ToLower()
if ($actualSha -ne $jarSha) {
    Remove-Item -Force $jar
    Write-Host "tla2tools.jar SHA-256 mismatch ($actualSha); deleted it, run again."; exit 2
}

$failed = $false
# Windows PowerShell turns a native program's stderr into errors; TLC's must not stop the run.
$ErrorActionPreference = 'Continue'
Push-Location $spec
try {
    foreach ($name in $expected.Keys) {
        $log = Join-Path $work "out\$name.txt"
        $meta = Join-Path $work "states\$name"
        if (Test-Path $meta) { Remove-Item -Recurse -Force $meta }
        $output = & java -XX:+UseParallelGC -cp $jar tlc2.TLC -nowarning -workers auto `
            -metadir $meta -config "$name.cfg" MC.tla 2>&1 | ForEach-Object { "$_" }
        $output | Set-Content -Encoding utf8 $log
        $text = $output -join "`n"
        if ($text -match 'Invariant (\w+) is violated') { $got = $Matches[1] }
        elseif ($text -match 'Model checking completed\. No error has been found') { $got = 'pass' }
        else { $got = 'error' }
        $states = if ($text -match '(\d+) distinct states found') { "$($Matches[1]) states" } else { '' }
        if ($got -eq $expected[$name]) {
            Write-Host ("OK    {0,-24} {1} {2}" -f $name, $got, $states)
        } else {
            $failed = $true
            Write-Host ("FAIL  {0,-24} expected {1}, got {2}  (full output: {3})" -f $name, $expected[$name], $got, $log)
        }
    }
} finally { Pop-Location }

if ($failed) { exit 1 }

# Drift: did the covered code change since the commit the model was last confirmed against?
$sync = (Select-String -Path $skill -Pattern '^Sync commit: `([0-9a-f]{7,40})`').Matches.Groups[1].Value
if (-not $sync) { Write-Host "No 'Sync commit' line in $skill."; exit 2 }
$changed = & git -C $root diff --name-only $sync -- @covered
if ($changed) {
    Write-Host "DRIFT: changed since sync commit ${sync}: $($changed -join ', ')"
    Write-Host 'Check the model against these changes before trusting the result (see SKILL.md).'
    exit 3
}
Write-Host "Covered code matches sync commit $sync."
exit 0
