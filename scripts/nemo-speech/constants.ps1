# =============================================================================
# NeMo-Speech.cpp diarization - shared constants and helpers (single source of
# truth). Dot-sourced by BOTH:
#   - scripts/nemo-speech/benchmark.ps1   (laptop benchmark, run by Faraz)
#   - .github/workflows/build-nemo-speech.yml (CI validation workflow)
#
# The diarize command string, absolute-path handling, model size/sha256, and the
# JSON-shape assertions live HERE so the CI smoke test and the laptop benchmark
# can never drift apart. If you change a constant, change it once, here.
#
# Verified 2026-09-30 (see PHASE1_SPEC.md). Do not "re-verify" these values.
# =============================================================================

# --- Model constants (verified 2026-09-30) -----------------------------------
$script:NEMO_MODEL_URL    = 'https://huggingface.co/nvidia/Nemotron-3-Diarization/resolve/main/Nemotron-3-Diarization.q8_0.gguf'
$script:NEMO_MODEL_SIZE   = 107012128
$script:NEMO_MODEL_SHA256 = '08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1'
$script:NEMO_MODEL_NAME   = 'Nemotron-3-Diarization.q8_0.gguf'

# The binary resolves input paths relative to its OWN directory, not the CWD.
# We therefore always hand it ABSOLUTE paths (see Get-NemoDiarizeArgs).
# `--preset v3-offline` is the V3 preset (larger streaming chunks/caches, right
# for long recordings). Do NOT use plain `--offline` (limited to ~6.6 min).
$script:NEMO_DEVICE = 'cpu'
$script:NEMO_PRESET = 'v3-offline'
$script:NEMO_FORMAT = 'json'

# --- Exact diarize command (the single source of truth) ----------------------
# Builds the argument vector for:
#   nemo-speech.exe diarize <abs.wav> --model <abs.gguf> --device cpu `
#                            --preset v3-offline --format json
function Get-NemoDiarizeArgs {
    param(
        [Parameter(Mandatory)][string]$Wav,
        [Parameter(Mandatory)][string]$Model
    )
    $wavAbs   = [System.IO.Path]::GetFullPath($Wav)
    $modelAbs = [System.IO.Path]::GetFullPath($Model)
    return @(
        'diarize', $wavAbs,
        '--model', $modelAbs,
        '--device', $script:NEMO_DEVICE,
        '--preset', $script:NEMO_PRESET,
        '--format', $script:NEMO_FORMAT
    )
}

# Runs the exact diarize command and returns the combined stdout/stderr text.
# (Used by the CI smoke test.)
function Invoke-NemoDiarize {
    param(
        [Parameter(Mandatory)][string]$Exe,
        [Parameter(Mandatory)][string]$Wav,
        [Parameter(Mandatory)][string]$Model
    )
    $exeAbs   = [System.IO.Path]::GetFullPath($Exe)
    $cmdArgs  = Get-NemoDiarizeArgs -Wav $Wav -Model $Model
    Write-Host "  CMD: $exeAbs $($cmdArgs -join ' ')"
    $out = & $exeAbs $cmdArgs 2>&1 | ForEach-Object { $_.ToString() } | Out-String
    return $out
}

# Pulls the JSON object out of possibly-noisy CLI output (progress text, etc.).
# Returns the substring from the first '{' to the last '}'.
function Get-JsonFromOutput {
    param([Parameter(Mandatory)][string]$Output)
    $text  = "$Output"
    $start = $text.IndexOf('{')
    $end   = $text.LastIndexOf('}')
    if ($start -lt 0 -or $end -lt $start) { return $text }
    return $text.Substring($start, $end - $start + 1)
}

# --- JSON shape assertion (mirrors the verified output shape) ----------------
# Verified shape: {"file":"<path>","segments":[{"start":0.000,"end":3.659,"speaker":1},...]}
# start/end in seconds, speaker integer 1-based.
# Sets $script:NemoJsonError and returns $true/$false.
function Test-NemoDiarizeJson {
    param([Parameter(Mandatory)][string]$Json)
    $script:NemoJsonError = ''

    $obj = $null
    try { $obj = $Json | ConvertFrom-Json }
    catch {
        $script:NemoJsonError = "JSON parse failed: $($_.Exception.Message)"
        return $false
    }

    if (-not $obj.PSObject.Properties.Name -contains 'segments') {
        $script:NemoJsonError = "JSON missing 'segments' field"
        return $false
    }

    $raw = $obj.segments
    if ($null -eq $raw) {
        $script:NemoJsonError = "'segments' is null"
        return $false
    }

    $segs = @($raw)
    if ($segs.Count -eq 0) {
        $script:NemoJsonError = "'segments' is empty"
        return $false
    }

    foreach ($s in $segs) {
        $props = @($s.PSObject.Properties.Name)
        if ($props -notcontains 'start' -or $props -notcontains 'end' -or $props -notcontains 'speaker') {
            $script:NemoJsonError = "Segment missing start/end/speaker: $($s | ConvertTo-Json -Compress)"
            return $false
        }
        if ($null -eq $s.start -or $null -eq $s.end -or $null -eq $s.speaker) {
            $script:NemoJsonError = 'Segment has null start/end/speaker'
            return $false
        }
        $startNum = [double]$s.start
        $endNum   = [double]$s.end
        $spkStr   = "$($s.speaker)"
        if ($spkStr -notmatch '^\d+$') {
            $script:NemoJsonError = "Speaker is not a non-negative integer: '$spkStr'"
            return $false
        }
        if ([int]$spkStr -lt 1) {
            $script:NemoJsonError = "Speaker must be >= 1: '$spkStr'"
            return $false
        }
        if (-not ($endNum -gt $startNum)) {
            $script:NemoJsonError = "end ($endNum) is not > start ($startNum)"
            return $false
        }
    }
    return $true
}

# --- Model verification (size + sha256) --------------------------------------
function Test-NemoModelFile {
    param([Parameter(Mandatory)][string]$Path)
    if (-not (Test-Path $Path)) {
        Write-Error "Model file not found: $Path"
        return $false
    }
    $len = (Get-Item $Path).Length
    if ($len -ne $script:NEMO_MODEL_SIZE) {
        Write-Error "Model size mismatch: expected $($script:NEMO_MODEL_SIZE), got $len"
        return $false
    }
    $sha = (Get-FileHash -Path $Path -Algorithm SHA256).Hash.ToLower()
    if ($sha -ne $script:NEMO_MODEL_SHA256) {
        Write-Error "Model sha256 mismatch: expected $($script:NEMO_MODEL_SHA256), got $sha"
        return $false
    }
    Write-Host "  Model verified: $len bytes, sha256 $sha"
    return $true
}

# --- WAV generation (PCM16, 16 kHz, mono) -----------------------------------
# Writes a WAV from a list of [durationSeconds, frequencyHz] segments where a
# frequency of 0 means silence. Uses 1-second tone blocks tiled across each
# segment so even a 30-minute file is generated quickly.
function Write-NemoTestWav {
    param(
        [Parameter(Mandatory)][string]$Path,
        [Parameter(Mandatory)][object[]]$Segments
    )
    $sampleRate = 16000
    $amplitude  = [int](0.3 * 32767)

    # Pre-generate one 1-second block per unique frequency (silence = zeros).
    $toneCache = @{}
    function Get-ToneBlock([int]$freq) {
        if ($freq -le 0) {
            return ,([System.Array]::new([int16], $sampleRate))
        }
        if ($toneCache.ContainsKey($freq)) { return ,$toneCache[$freq] }
        $block = [System.Array]::new([int16], $sampleRate)
        for ($i = 0; $i -lt $sampleRate; $i++) {
            $block[$i] = [int16]($amplitude * [Math]::Sin(2.0 * [Math]::PI * $freq * $i / $sampleRate))
        }
        $toneCache[$freq] = $block
        return ,$block
    }

    # Total sample count, then tile the 1-second blocks into the full buffer.
    $totalSamples = 0
    foreach ($s in $Segments) { $totalSamples += [int]([double]$s[0] * $sampleRate) }
    $pcm = [System.Array]::new([int16], $totalSamples)
    $offset = 0
    foreach ($s in $Segments) {
        $dur    = [double]$s[0]
        $freq   = [int]$s[1]
        $block  = Get-ToneBlock $freq
        $whole  = [int]$dur
        $frac   = [int](($dur - $whole) * $sampleRate)
        for ($b = 0; $b -lt $whole; $b++) {
            [Array]::Copy($block, 0, $pcm, $offset, $sampleRate)
            $offset += $sampleRate
        }
        if ($frac -gt 0) {
            [Array]::Copy($block, 0, $pcm, $offset, $frac)
            $offset += $frac
        }
    }

    # int16[] -> little-endian byte[] (raw memory copy; x86 is little-endian).
    $data = [System.Array]::new([byte], $pcm.Length * 2)
    [System.Buffer]::BlockCopy($pcm, 0, $data, 0, $data.Length)

    # Write the 44-byte WAV header + data.
    $fs = [System.IO.File]::Create($Path)
    $bw = [System.IO.BinaryWriter]::new($fs)
    $bw.Write([byte[]](0x52, 0x49, 0x46, 0x46))   # 'RIFF'
    $bw.Write([int32]($data.Length + 36))          # RIFF chunk size
    $bw.Write([byte[]](0x57, 0x41, 0x56, 0x45))   # 'WAVE'
    $bw.Write([byte[]](0x66, 0x6D, 0x74, 0x20))   # 'fmt '
    $bw.Write([int32]16)                           # fmt chunk size
    $bw.Write([int16]1)                           # audio format = PCM
    $bw.Write([int16]1)                           # channels = mono
    $bw.Write([int32]$sampleRate)                 # sample rate
    $bw.Write([int32]($sampleRate * 2))           # byte rate
    $bw.Write([int16]2)                           # block align
    $bw.Write([int16]16)                         # bits per sample
    $bw.Write([byte[]](0x64, 0x61, 0x74, 0x61))  # 'data'
    $bw.Write([int32]$data.Length)               # data size
    $bw.Write($data)
    $bw.Dispose()
    $fs.Dispose()
}
