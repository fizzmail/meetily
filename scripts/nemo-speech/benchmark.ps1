<#
.SYNOPSIS
    NeMo-Speech.cpp diarization benchmark for the target laptop (Faraz's i5-1245U).

.DESCRIPTION
    Measures real RTF (real-time factor) of the Nemotron 3 diarization model on
    30 s / 5 min / 30 min recordings, plus peak working set (to confirm the
    ~170 MB RAM figure). Runs the EXACT diarize command (absolute paths) from the
    shared constants.ps1 module, so it is identical to the CI smoke test.

    Self-contained: copy this script + constants.ps1 + the sidecar bundle anywhere
    on the laptop and run:

        powershell -ExecutionPolicy Bypass -File .\benchmark.ps1 `
            -BundleDir "C:\nemo-speech-bundle" `
            -ModelPath "C:\models\Nemotron-3-Diarization.q8_0.gguf"

    No admin, no toolchain, no repo-relative assumptions beyond this directory.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$BundleDir,
    [Parameter(Mandatory)][string]$ModelPath
)

# --- Load the shared single-source-of-truth module -------------------------
$constantsPath = Join-Path $PSScriptRoot 'constants.ps1'
if (-not (Test-Path $constantsPath)) {
    Write-Error "constants.ps1 not found next to benchmark.ps1: $constantsPath"
    Write-Error "Copy scripts/nemo-speech/constants.ps1 alongside benchmark.ps1 and re-run."
    exit 1
}
. $constantsPath

$ErrorActionPreference = 'Stop'

# --- Locate the sidecar binary --------------------------------------------
$exe = Join-Path $BundleDir 'nemo-speech.exe'
if (-not (Test-Path $exe)) {
    Write-Error "nemo-speech.exe not found in bundle: $exe"
    exit 1
}

# --- Keep the model out of the default cache ------------------------------
$modelDir = Join-Path ([System.IO.Path]::GetTempPath()) ("nemo-bench-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $modelDir | Out-Null
$env:NEMO_SPEECH_MODEL_DIR = $modelDir
Write-Host "NEMO_SPEECH_MODEL_DIR = $modelDir"

# --- Verify the model (size + sha256, shared constants) -------------------
Write-Host "`n=== Verifying model ==="
if (-not (Test-NemoModelFile -Path $ModelPath)) {
    Write-Error "Model verification failed."
    exit 1
}

# --- Test audio generation ------------------------------------------------
# 30 s: 10 s 440 Hz, 5 s silence, 10 s 880 Hz, 5 s silence (matches CI smoke test).
# 5 min / 30 min: alternating 10 s tone (440/880) / 10 s silence blocks to mimic
# speaker changes.
function New-AlternatingSegments([double]$totalSeconds) {
    $segs = [System.Collections.Generic.List[object]]::new()
    $t = 0.0
    $block = 0
    while ($t -lt $totalSeconds) {
        $len = [Math]::Min(10.0, ($totalSeconds - $t))
        if ($block % 2 -eq 0) {
            $freq = if (([int]($block / 2)) % 2 -eq 0) { 440 } else { 880 }
        } else {
            $freq = 0
        }
        $pair = @($len, $freq)
        $segs.Add($pair)
        $t += $len
        $block++
    }
    return ,$segs.ToArray()
}

$workDir = Join-Path ([System.IO.Path]::GetTempPath()) ("nemo-bench-wav-" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $workDir | Out-Null

$cases = @(
    @{ Name = '30s';  Seconds = 30;   Segments = @( @(10,440), @(5,0), @(10,880), @(5,0) ) }
    @{ Name = '5min'; Seconds = 300;  Segments = (New-AlternatingSegments 300) }
    @{ Name = '30min';Seconds = 1800; Segments = (New-AlternatingSegments 1800) }
)

Write-Host "`n=== Generating test WAVs (16 kHz mono PCM16) ==="
foreach ($c in $cases) {
    $wavPath = Join-Path $workDir ("nemo-test-" + $c.Name + ".wav")
    Write-NemoTestWav -Path $wavPath -Segments $c.Segments
    $c['WavPath'] = $wavPath
    Write-Host ("  {0,-6} -> {1}  ({2:N0} bytes)" -f $c.Name, $wavPath, (Get-Item $wavPath).Length)
}

# --- Timed diarize run + peak working set --------------------------------
# Uses Start-Process so we can sample the child's working set while it runs.
# The command itself comes from the shared Get-NemoDiarizeArgs (identical to CI).
function Invoke-NemoDiarizeTimed {
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][string]$Wav,
        [Parameter(Mandatory)][string]$Model
    )
    $exeAbs   = [System.IO.Path]::GetFullPath($Exe)
    $cmdArgs  = Get-NemoDiarizeArgs -Wav $Wav -Model $Model
    $outFile  = [System.IO.Path]::ChangeExtension($Wav, '.json')
    $errFile  = [System.IO.Path]::ChangeExtension($Wav, '.err')
    $peakWs   = 0

    $elapsed = Measure-Command {
        $p = Start-Process -FilePath $exeAbs -ArgumentList $cmdArgs -PassThru `
            -RedirectStandardOutput $outFile -RedirectStandardError $errFile
        while (-not $p.HasExited) {
            Start-Sleep -Milliseconds 100
            try {
                $ws = (Get-Process -Id $p.Id -ErrorAction Stop).WorkingSet64
                if ($ws -gt $peakWs) { $peakWs = $ws }
            } catch { }
        }
        $p.WaitForExit()
    }

    $raw = ''
    if (Test-Path $outFile) { $raw = Get-Content $outFile -Raw -ErrorAction SilentlyContinue }
    return @{ Elapsed = $elapsed; PeakWorkingSet = $peakWs; Output = $raw; ExitCode = $null }
}

# --- Run all cases --------------------------------------------------------
Write-Host "`n=== Running diarization benchmark ==="
$results = @()
$anyFailed = $false

foreach ($c in $cases) {
    Write-Host "`n--- $($c.Name) ($($c.Seconds)s) ---"
    $r = Invoke-NemoDiarizeTimed -Exe $exe -Wav $c.WavPath -Model $ModelPath
    $json = Get-JsonFromOutput -Output $r.Output

    $ok = Test-NemoDiarizeJson -Json $json
    if (-not $ok) {
        $anyFailed = $true
        Write-Error "JSON shape assertion FAILED for $($c.Name): $script:NemoJsonError"
        Write-Error "Raw output: $json"
    }

    $segCount = 0
    $spkCount = 0
    if ($ok) {
        $obj = $json | ConvertFrom-Json
        $segs = @($obj.segments)
        $segCount = $segs.Count
        $spkCount = ($segs | ForEach-Object { $_.speaker } | Sort-Object -Unique).Count
    }

    $rtf = 0.0
    if ($c.Seconds -gt 0) { $rtf = $r.Elapsed.TotalSeconds / $c.Seconds }

    $results += [pscustomobject]@{
        Name          = $c.Name
        Seconds       = $c.Seconds
        WallSeconds   = [math]::Round($r.Elapsed.TotalSeconds, 2)
        Rtf           = [math]::Round($rtf, 4)
        Segments      = $segCount
        Speakers      = $spkCount
        PeakMb        = [math]::Round($r.PeakWorkingSet / 1MB, 1)
        JsonOk        = $ok
    }

    Write-Host ("  wall={0:N2}s  rtf={1:N4}  segments={2}  speakers={3}  peak={4:N1} MB  json_ok={5}" -f `
        $r.Elapsed.TotalSeconds, $rtf, $segCount, $spkCount, ($r.PeakWorkingSet / 1MB), $ok)
}

# --- Report ---------------------------------------------------------------
$table = $results | Format-Table -AutoSize | Out-String
Write-Host "`n=== Benchmark results ==="
Write-Host $table

$peakMb = ($results | Measure-Object -Property PeakMb -Maximum).Maximum
$passFail = if ($anyFailed) { 'FAILED' } else { 'PASSED' }
$summary = @"
NeMo-Speech.cpp diarization benchmark
Host: $env:COMPUTERNAME
Date: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss')
Bundle: $exe
Model:  $ModelPath

$table
Peak working set (max across runs): {0:N1} MB  (expected ~170 MB)
JSON shape assertions: {1}
"@ -f $peakMb, $passFail

Write-Host $summary

$outFile = Join-Path $PSScriptRoot 'benchmark-results.txt'
$summary | Set-Content -Path $outFile -Encoding utf8
Write-Host "Wrote results to: $outFile"

# --- Exit code ------------------------------------------------------------
if ($anyFailed) {
    Write-Error "Benchmark FAILED: at least one JSON shape assertion failed (see above)."
    exit 1
}
exit 0
