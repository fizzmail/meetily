# Phase 1: NeMo-Speech.cpp Windows sidecar build (CI validation + laptop benchmark)

## Goal
De-risk the diarization toolchain BEFORE any app integration code. Two deliverables:
1. A NEW GitHub Actions workflow that builds NeMo-Speech.cpp `main` (CPU) on `windows-latest`, smoke-tests the Nemotron 3 model end-to-end, and uploads the sidecar bundle as an artifact for inspection.
2. A laptop benchmark script (PowerShell) that Faraz runs on his i5-1245U to measure real RTF.

**HARD CONSTRAINTS:**
- Do NOT commit or push anything. Leave all changes uncommitted in the working tree.
- Do NOT modify `frontend/src-tauri/tauri.conf.json`, any app code, or the existing `build-windows.yml`. This phase is CI + tooling ONLY — the app integration is Phase 2.
- New files only: `.github/workflows/build-nemo-speech.yml` and `scripts/nemo-speech/benchmark.ps1` (plus any helper scripts under `scripts/nemo-speech/`).
- `frontend/src/app/layout.tsx` is a preview-bypass file — never touch it.

## Verified facts (from Phase 0, 2026-09-30 — do not re-verify, use as-is)
- **Repo:** https://github.com/NVIDIA/NeMo-Speech.cpp, branch `main` at commit `4c101bc` (as of 2026-09-30; build from `main` HEAD — Nemotron 3 support landed 2026-09-24 via PR #50/#52 and is UNRELEASED; the v0.1.0 prebuilt release CANNOT run the model: `sortformer: pre_ln transformer variant is not supported`).
- **Submodules:** `ggml`, `llama.cpp`, `third_party/cpp-httplib` (`git submodule update --init`).
- **Windows build driver:** `scripts\windows\build.ps1 -Backend cpu` (MSVC vcvars64 + Ninja + vcpkg manifest auto-bootstrap under `%LOCALAPPDATA%\NeMoSpeech`). The CMake preset for a minimal diarization build is `cpu-diar` (see `CMakePresets.json` — "CPU standalone diarization"). The driver supports `-Profile asr` / `-AsrOnly` for a minimal CLI+ASR+diarization build — prefer the most minimal profile that still produces the `nemo-speech` CLI with the `diarize` command; if the driver's profile flags don't map cleanly, fall back to a raw CMake configure with the `cpu-diar` preset (`cmake --preset cpu-diar` equivalent, Ninja generator, Release).
- **Model:** `https://huggingface.co/nvidia/Nemotron-3-Diarization/resolve/main/Nemotron-3-Diarization.q8_0.gguf` — **107,012,128 bytes**, SHA-256 `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` (verified 2026-09-30). 100M-param Sortformer, up to 8 speakers (cap, model estimates count), 16 kHz mono input, ~170 MB RAM, ~RTF 0.04–0.05 on a modern i5 (TO BE BENCHMARKED on the target laptop — that's deliverable 2).
- **The exact command (verified working on the Linux source build):**
  `nemo-speech diarize /abs/path/tmp.wav --model /abs/path/nemotron3.gguf --device cpu --preset v3-offline --format json`
  - `--preset v3-offline` is the V3 preset (larger streaming chunks/caches — right for long recordings). Do NOT use plain `--offline` (limited to ~6.6 min by positional table).
  - **CRITICAL: the binary resolves input paths relative to its OWN directory, not the CWD — always pass ABSOLUTE paths** for both the WAV and `--model`.
  - **Verified JSON output shape:** `{"file": "<path>", "segments": [{"start": 0.000, "end": 3.659, "speaker": 1}, ...]}` — start/end in seconds (3 decimals), speaker integer 1-based.
- **Shared-library deps (from the verified Linux `ldd`; Windows equivalents are the .dlls in the build `bin/` dir):** `nemo-speech.exe` needs `ggml.dll`/`ggml-base.dll`/`ggml-cpu.dll` (or versioned variants), `nemo_speech_asr.dll` / `nemo_speech_asr_c.dll` (or versioned), plus the MSVC OpenMP runtime **`vcomp140.dll`** (must sit alongside `nemo-speech.exe`; it's in the v0.1.0 release zip but the source build needs it added).
- **Model cache:** `%LOCALAPPDATA%\NeMoSpeech\models` by default; override with env `NEMO_SPEECH_MODEL_DIR`.
- `nemo-speech model list` shows the diarization short names (run it in the smoke test to capture the exact short name for the `pull` fallback path — record it in the workflow summary).
- The existing app sidecar pattern (for Phase 2 reference, NOT to be touched now): `tauri.conf.json` `externalBin: ["binaries/llama-helper", "binaries/ffmpeg"]`, built in `build-windows.yml` and copied to `frontend/src-tauri/binaries/`.

## Deliverable 1: `.github/workflows/build-nemo-speech.yml`

A standalone, manually-triggered workflow (do NOT wire it into `build-windows.yml` this phase — keep it inspectable in isolation).

```
name: "Build NeMo-Speech sidecar (validation)"
on:
  workflow_dispatch:
    inputs:
      upload-artifacts:
        description: 'Upload sidecar bundle artifact'
        required: true
        type: boolean
        default: true
```

Job `build-nemo` on `windows-latest` (VS2022 is present by default on the runner — verify with `vswhere` / `cl --version` in a first step and fail loudly if absent):

1. **Checkout** `NVIDIA/NeMo-Speech.cpp` @ `main` (`actions/checkout@v4`, `submodules: recursive`).
2. **Toolchain check:** confirm `cmake` (≥3.26), `ninja`, MSVC `cl` (vcvars64), `git` — the build script checks these itself, but a pre-check gives a clearer failure.
3. **Build:** `powershell -ExecutionPolicy Bypass -File scripts\windows\build.ps1 -Backend cpu` with the most minimal profile that yields the `nemo-speech` CLI + `diarize` (try `-Profile asr` or `-AsrOnly` first; fall back to the `cpu-diar` CMake preset if the driver's flags don't map). Capture the full build log.
4. **Build output inventory (workflow summary):** list the build `bin/` directory contents with sizes; explicitly assert presence of `nemo-speech.exe` and every `ggml*`/`nemo_speech_asr*` DLL the exe needs. Check the exe's imports (e.g. `dumpbin /DEPENDENTS` or PowerShell `System.Diagnostics.Process` load test) to confirm `vcomp140.dll` is required; if required and not in `bin/`, copy it from the VC redist (`vc\Redist\MSVC\<ver>\x64\` or the vcpkg/VC install) into the bundle.
5. **Smoke test (the core of this phase):**
   a. Download the GGUF with `curl -L`, verify size = 107012128 AND sha256 = `08456d9e22cd9a323c0364d98375f3746d6e68507ebb705cd46438c534c7a3a1` — fail the workflow on mismatch.
   b. Generate a 30-second 16 kHz mono WAV test file. The runner has no guaranteed ffmpeg — use PowerShell to write a PCM16 WAV directly (a short script under `scripts/nemo-speech/` in THIS repo is fine, or inline): 10 s tone at 440 Hz, silence, 10 s tone at 880 Hz, silence. (Tones are fine — the goal is to exercise the full model path and JSON output, not to get meaningful speaker labels.)
   c. Run the EXACT command with absolute paths (PowerShell, `NEMO_SPEECH_MODEL_DIR` pointed at a temp dir to keep the model out of the default cache):
      `& $bin\nemo-speech.exe diarize $wav --model $gguf --device cpu --preset v3-offline --format json`
   d. Parse the JSON from stdout; assert: `segments` is a non-empty array, every segment has numeric `start`/`end` and integer `speaker` ≥ 1, and `end > start`. Print the full JSON to the workflow summary.
   e. Also run `nemo-speech model list` and record the diarization short name(s) in the summary (for the Phase 2 `pull` fallback path).
6. **Sidecar bundle:** assemble a `nemo-speech-bundle/` directory = `nemo-speech.exe` + all required DLLs from `bin/` + `vcomp140.dll`. Upload as an artifact (`nemo-speech-sidecar-windows-x64`, retention 30 days) when `upload-artifacts` is true.
7. **Workflow summary:** build duration, toolchain versions (cmake/ninja/cl), bundle file list with sizes, smoke-test JSON, model short name.

Caching: cache the vcpkg installed dir and the build dir keyed on the NeMo-Speech.cpp commit SHA (the repo is checked out at `main` HEAD, so key on `hashFiles` of the checked-out `CMakePresets.json` + the submodule SHAs, or use `actions/cache` with a key from `git rev-parse HEAD` in the nemo checkout). Keep it simple — a cache miss must never fail the build.

## Deliverable 2: `scripts/nemo-speech/benchmark.ps1` (laptop, run by Faraz)

A single PowerShell script, runnable on his Windows laptop (no admin, no toolchain assumptions beyond "the sidecar bundle exists"), that:
1. Takes `-BundleDir` (path to the extracted sidecar bundle) and `-ModelPath` (path to the GGUF) as parameters.
2. Sets `NEMO_SPEECH_MODEL_DIR` to a temp dir, copies/verifies the GGUF (size + sha256 check, same constants as above).
3. Generates 3 test WAVs with inline PowerShell PCM16 writing: 30 s, 5 min, 30 min (16 kHz mono; tones + silence, alternating segments to mimic speaker changes).
4. Runs the exact diarize command (absolute paths) on each, timing wall-clock with `Measure-Command`.
5. Prints a table: duration, wall time, RTF (wall/duration), and the segment count + distinct speaker count per file. Writes the same to `benchmark-results.txt` next to the script.
6. Also reports process peak working set (via `Get-Process` sampling or the .NET `Process` class) to confirm the ~170 MB RAM figure.
7. Fails loudly (non-zero exit + clear message) if the JSON shape assertion fails, mirroring the CI smoke test.

The script must be self-contained (no repo-relative assumptions beyond its own directory) so Faraz can copy it + the bundle anywhere on the laptop.

## Acceptance criteria (verify before reporting done)
1. `build-nemo-speech.yml` passes YAML lint (e.g. `python -c "import yaml; yaml.safe_load(...)"` or `yamllint` if available) and the job graph is correct (single job, steps in order).
2. `benchmark.ps1` passes `powershell -NoProfile -Command "Get-Content ... | Out-Null"` parse check if a pwsh parser is available locally; otherwise a careful manual review + `pwsh -NoProfile -File scripts/nemo-speech/benchmark.ps1 -?` style syntax check if `pwsh` exists on this box.
3. The exact diarize command string, absolute-path handling, sha256/size constants, and JSON-shape assertions are IDENTICAL between the workflow and the benchmark script (single source of truth — put the shared constants in a small `scripts/nemo-speech/constants.ps1` that both reference, or duplicate them verbatim and note it).
4. `git status` shows ONLY the new files (workflow + scripts) — no app code, no tauri.conf.json, no layout.tsx, nothing committed.
5. Report back: the exact build command used (driver flags or raw CMake), whether `-Profile asr`/`-AsrOnly` mapped cleanly or the `cpu-diar` preset fallback was needed, the bundle file list, and any surprises (e.g. vcpkg bootstrap behavior, missing tools on the runner).

## Out of scope (Phase 2)
- App code: settings UI, model download flow, "Assign Speakers" command, segment→transcript matching, speaker naming/merge UI.
- `tauri.conf.json` `externalBin` addition.
- Wiring the sidecar build into `build-windows.yml`.
