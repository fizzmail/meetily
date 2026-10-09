# Diarization Run Progress — Phase Labels + Elapsed Time (spec for OpenCode)

## Goal
Replace the static "Analyzing speakers… this can take a few minutes" spinner in the
diarization dialog with **honest phase labels** and a **live elapsed-time counter**.
No progress percentage, no duration estimate (the model/CLI exposes neither — the
sidecar `nemo-speech diarize` is one synchronous blocking call with no progress
hook; see references/diarization.md). The user should be able to tell "it's working
and in which phase" from the UI.

This is a **small Rust change (3 event emits) + a frontend change (listen + timer +
UI)**. No new Rust dependencies, no sidecar/CLI changes, no DB changes.

## The three phases (honest, in execution order)
1. `preparing`  — model check + audio locate + ffmpeg 16 kHz mono conversion (fast, seconds)
2. `analyzing`  — the sidecar inference (the long part; minutes)
3. `finalizing` — segment→row matching + DB persist (fast)

Frontend label map (use EXACTLY these strings):
- `preparing`  → `Preparing audio…`
- `analyzing`  → `Analyzing speakers…`
- `finalizing` → `Finalizing…`

## Event contract (do NOT rename)
- Event name: `diarization-run-phase`
- Payload: `{ "phase": "preparing" | "analyzing" | "finalizing" }`
- Emitted via the existing `Emitter` trait: `app.emit("diarization-run-phase", serde_json::json!({ "phase": "analyzing" }))`
  (matches the existing `diarization-model-download-progress` emit pattern in the same file)

## Rust change — `frontend/src-tauri/src/diarization/commands.rs`, `run_diarization`
`app: AppHandle<R>` is already a parameter and `Emitter` is already imported
(`use tauri::{AppHandle, Emitter, Manager, Runtime, State};`). `app` is not moved
after the inner block, so it can be borrowed by the inner `async { ... }.await`
closure. Add exactly three `let _ = app.emit(...)` calls at these points (ignore the
result, matching the existing emit style):

1. **`preparing`** — inside the inner `async` block, as the FIRST statement, before
   `convert_to_wav(...)` (step 4).
2. **`analyzing`** — inside the inner block, immediately BEFORE
   `sidecar::run_diarization(...)` (step 5).
3. **`finalizing`** — inside the inner block, immediately BEFORE the persist block
   (step 8, before `repository::save_transcript_speaker_assignments(...)`).

Do NOT add `async move` to the closure (it currently captures `pool`, `app_data_dir`,
`wav_path`, `model_path`, `started_at` by reference; `app` borrows the same way).

## Frontend change — `frontend/src/components/MeetingDetails/SpeakerDiarization.tsx`
Add imports (match the existing pattern in `DiarizationSettings.tsx` /
`RetranscribeDialog.tsx`):
```ts
import { listen, UnlistenFn } from "@tauri-apps/api/event";
```

Add state:
```ts
type Phase = "preparing" | "analyzing" | "finalizing";
const [phase, setPhase] = useState<Phase>("preparing");
const [elapsedSec, setElapsedSec] = useState(0);
const startedAtRef = useRef<number | null>(null);
```

1. **Phase listener** — a `useEffect` on mount that subscribes and cleans up:
   ```ts
   useEffect(() => {
     let unlisten: UnlistenFn | null = null;
     listen<{ phase: Phase }>("diarization-run-phase", (e) => {
       if (e.payload && e.payload.phase) setPhase(e.payload.phase);
     }).then((u) => { unlisten = u; });
     return () => { unlisten?.(); };
   }, []);
   ```

2. **Elapsed timer** — a `useEffect` gated on `isRunning`:
   ```ts
   useEffect(() => {
     if (!isRunning) return;
     startedAtRef.current = Date.now();
     setElapsedSec(0);
     const id = setInterval(() => {
       if (startedAtRef.current !== null) {
         setElapsedSec(Math.floor((Date.now() - startedAtRef.current) / 1000));
       }
     }, 1000);
     return () => { clearInterval(id); startedAtRef.current = null; };
   }, [isRunning]);
   ```
   (Start the clock when the run starts; it naturally covers all three phases and
   stops when the invoke resolves/rejects because `isRunning` flips to false in the
   existing `finally`.)

3. **Label helper** (module scope, near `colorFor`):
   ```ts
   const PHASE_LABEL: Record<Phase, string> = {
     preparing: "Preparing audio…",
     analyzing: "Analyzing speakers…",
     finalizing: "Finalizing…",
   };
   function formatElapsed(sec: number): string {
     const m = Math.floor(sec / 60);
     const s = sec % 60;
     return `${m}:${s.toString().padStart(2, "0")}`;
   }
   ```

4. **UI** — in the `isRunning ? (...)` branch, keep the `Loader2` spinner and replace
   the static `<p>Analyzing speakers… this can take a few minutes</p>` with the phase
   label + elapsed time:
   ```tsx
   <p className="text-sm text-muted-foreground">{PHASE_LABEL[phase]}</p>
   <p className="text-xs text-muted-foreground tabular-nums">{formatElapsed(elapsedSec)}</p>
   ```

## Do NOT change
- The sidecar command, `nemo-speech` CLI, model download flow, or DB schema.
- The `run_diarization` success/error/`finally` flow (the timer is driven by the
  existing `isRunning` state).
- Any Rust `Cargo.toml` dependency.

## Verification (run on this box before declaring done)
1. `cd frontend/src-tauri && cargo check -p meetily` — must be clean. (Needs the
   gitignored sidecar stub: `touch frontend/src-tauri/binaries/nemo-speech-x86_64-unknown-linux-gnu`.)
2. `cd frontend && npx tsc --noEmit` — must be clean.
3. Grep confirm: exactly three `app.emit("diarization-run-phase"` sites in
   `commands.rs`; the `listen("diarization-run-phase"` site + `PHASE_LABEL` +
   `formatElapsed` in `SpeakerDiarization.tsx`.
4. Confirm NO `async move` was added to the `run_diarization` inner block and NO new
   Rust dependency was added (`git diff --stat` on `Cargo.toml` must be empty).
