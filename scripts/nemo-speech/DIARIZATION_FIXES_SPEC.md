# Diarization UX Fixes — Refetch + Re-Assign Hint + Phase/Elapsed Progress
### Spec for OpenCode (3 sequential one-shots; verify each before the next)

All three fixes touch the same two areas (the `run_diarization` flow + the
diarization dialog). They are implemented as **three separate one-shots** so each
can be verified independently. No new Rust dependencies, no sidecar/CLI changes,
no DB schema changes.

Shared context (applies to all three):
- Rust command: `run_diarization` in `frontend/src-tauri/src/diarization/commands.rs`.
  It already takes `app: AppHandle<R>` and `Emitter` is already imported
  (`use tauri::{AppHandle, Emitter, Manager, Runtime, State};`). The inner work runs
  in `let result: ... = async { ... }.await;` — `app` is borrowed by that closure
  and NOT moved, so `app.emit(...)` works inside it. Do NOT add `async move`.
- Frontend component: `frontend/src/components/MeetingDetails/SpeakerDiarization.tsx`
  (`SpeakerDiarizationControl`). Props currently: `{ meetingId, onSpeakersChange }`.
- `page-content.tsx` renders it (around line 224) as
  `<SpeakerDiarizationControl meetingId={meeting.id} onSpeakersChange={setSpeakerNameMap} />`.
  `onRefetchTranscripts` is already in scope in `page-content.tsx` (it is passed to
  `TranscriptPanel`/`TranscriptButtonGroup` in the same component).
- Existing event pattern to copy: `app.emit("diarization-model-download-progress",
  serde_json::json!({ ... }))` (same file). Frontend `listen` pattern to copy:
  `import { listen, UnlistenFn } from "@tauri-apps/api/event"` (see
  `DiarizationSettings.tsx` / `RetranscribeDialog.tsx`).

---

## FIX 1 — Refetch transcripts after diarization so badges appear without a page reload
Root cause: after `run_diarization` returns, the in-memory transcript segments still
have `speaker_id = undefined` (they were loaded before the run). The DB is correct,
but the UI never re-reads it, so badges only appear on a page reload.

Changes:
1. `SpeakerDiarization.tsx` — add a prop to `SpeakerDiarizationControlProps`:
   ```ts
   onRefetchTranscripts?: () => Promise<void>;
   ```
   and destructure it in the component signature.
2. `page-content.tsx` — pass it through where the component is rendered:
   ```tsx
   <SpeakerDiarizationControl
     meetingId={meeting.id}
     onSpeakersChange={setSpeakerNameMap}
     onRefetchTranscripts={onRefetchTranscripts}
   />
   ```
3. In `runDiarization`, inside the `try`, AFTER the success `toast.success(...)`
   (i.e. after `setSpeakers(result)` / `publishNames(result)`), add:
   ```ts
   if (onRefetchTranscripts) { await onRefetchTranscripts(); }
   ```
4. In `handleMerge`, inside its `try`, AFTER the success `toast.success("Speakers
   merged")`, add the same refetch (merge reassigns transcript rows, so the in-memory
   segments' `speaker_id` is stale):
   ```ts
   if (onRefetchTranscripts) { await onRefetchTranscripts(); }
   ```
   (Do NOT add a refetch to `handleRename` — rename only changes the registry display
   name, not row assignments; the name map is already updated via `publishNames`.)

Verify: `npx tsc --noEmit` clean; grep shows `onRefetchTranscripts` in the props
interface, the destructure, `page-content.tsx`, and exactly two call sites
(`runDiarization` + `handleMerge`).

---

## FIX 2 — Re-Assign hint (make a re-run a conscious choice)
Context: re-running "Assign Speakers" re-does the full pipeline (ffmpeg + full model
inference, minutes) with no indication that it's a re-run. Make the re-run explicit.

Changes (all in `SpeakerDiarization.tsx`):
1. The main "Assign Speakers" button (the one with `onClick={runDiarization}`,
   `title="Assign speakers to the transcript"`): make the label + title reflect a
   re-run when speakers already exist:
   ```tsx
   <span className="hidden @[22rem]:inline">
     {speakers.length > 0 ? "Re-Assign Speakers" : "Assign Speakers"}
   </span>
   ```
   and set `title={speakers.length > 0 ? "Re-analyze this meeting's speakers" : "Assign speakers to the transcript"}`.
2. In the `isRunning ? (...)` branch, AFTER the phase label + elapsed-time (see FIX 3),
   add a re-run note that shows only when this is a re-run. A re-run is detected by
   `speakers.length > 0` at run time (fresh runs start with an empty `speakers` array;
   `setSpeakers(result)` only happens at the end, so during the run `speakers.length > 0`
   means a previous run already assigned speakers):
   ```tsx
   {speakers.length > 0 && (
     <p className="text-xs text-muted-foreground">
       Re-run — re-analyzing the whole meeting
     </p>
   )}
   ```

Verify: `npx tsc --noEmit` clean; grep shows `"Re-Assign Speakers"` and the re-run
note string.

---

## FIX 3 — Phase labels + live elapsed time (no percentage, no estimate)
Context: the sidecar `nemo-speech diarize` is one synchronous blocking call with no
progress hook, so a real percentage is impossible. Show honest phase labels + a live
elapsed-time counter so the user can tell "it's working, in which phase, and how long
it's been running."

Event contract (do NOT rename):
- Event name: `diarization-run-phase`
- Payload: `{ "phase": "preparing" | "analyzing" | "finalizing" }`

Rust change — `run_diarization`, add exactly three `let _ = app.emit(...)` calls
(ignore the result, matching the existing emit style):
1. `preparing` — first statement inside the inner `async { ... }` block, before
   `convert_to_wav(...)` (step 4).
2. `analyzing` — immediately BEFORE `sidecar::run_diarization(...)` (step 5).
3. `finalizing` — immediately BEFORE the persist block (step 8, before
   `repository::save_transcript_speaker_assignments(...)`).

```rust
let _ = app.emit("diarization-run-phase", serde_json::json!({ "phase": "preparing" }));
```
(and `analyzing`, `finalizing` analogously).

Frontend change — `SpeakerDiarization.tsx`:
1. Import: `import { listen, UnlistenFn } from "@tauri-apps/api/event";`
2. Add state + refs:
   ```ts
   type Phase = "preparing" | "analyzing" | "finalizing";
   const [phase, setPhase] = useState<Phase>("preparing");
   const [elapsedSec, setElapsedSec] = useState(0);
   const startedAtRef = useRef<number | null>(null);
   ```
3. Phase listener (mount effect, with cleanup):
   ```ts
   useEffect(() => {
     let unlisten: UnlistenFn | null = null;
     listen<{ phase: Phase }>("diarization-run-phase", (e) => {
       if (e.payload && e.payload.phase) setPhase(e.payload.phase);
     }).then((u) => { unlisten = u; });
     return () => { unlisten?.(); };
   }, []);
   ```
4. Elapsed timer (gated on `isRunning`; the existing `finally` flips `isRunning` to
   false, which stops the clock):
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
5. Module-scope helpers (near `colorFor`):
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
6. UI — in the `isRunning ? (...)` branch, keep the `Loader2` spinner and replace the
   static `<p>Analyzing speakers… this can take a few minutes</p>` with the phase
   label + elapsed time (FIX 2's re-run note goes after these):
   ```tsx
   <p className="text-sm text-muted-foreground">{PHASE_LABEL[phase]}</p>
   <p className="text-xs text-muted-foreground tabular-nums">{formatElapsed(elapsedSec)}</p>
   ```

Verify:
- `cd frontend/src-tauri && cargo check -p meetily` clean (needs the gitignored stub
  `touch frontend/src-tauri/binaries/nemo-speech-x86_64-unknown-linux-gnu`).
- `cd frontend && npx tsc --noEmit` clean.
- Grep: exactly three `app.emit("diarization-run-phase"` sites in `commands.rs`;
  `listen("diarization-run-phase"` + `PHASE_LABEL` + `formatElapsed` in
  `SpeakerDiarization.tsx`.
- `git diff --stat` on `Cargo.toml` must be empty (no new Rust dependency).
- Confirm NO `async move` was added to the `run_diarization` inner block.

---

## Do NOT change (all fixes)
- The `nemo-speech` sidecar command, CLI, model download flow, or DB schema.
- The `run_diarization` success/error/`finally` flow (the elapsed timer is driven by
  the existing `isRunning` state).
- `layout.tsx` (stays uncommitted by design).
