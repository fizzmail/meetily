# Audio Clip Player Fix — stale onended + play-before-ready
### Spec for OpenCode (single one-shot; frontend-only, no Rust changes)

## Symptom (user-reported)
In the Assign Speakers dialog, the "Listen" clip player: sometimes only a quick
burst plays and stops; the play button must be pressed a few times to hear the
whole clip. Clips save correctly (the Rust `get_speaker_clip` side is clean).

## Root causes (verified in `frontend/src/hooks/useAudioPlayer.ts`)

**Race 1 — stale `onended` kills the NEXT source (burst-then-stop).**
In `play()`: `stopPlayback()` stops the current source, then a new source is
created and started. Per the Web Audio spec, a stopped source's `onended` fires
**asynchronously** (next task boundary), i.e. AFTER the new source has started.
The handler is:
```ts
sourceRef.current.onended = () => { stopPlayback(); setCurrentTime(0); };
```
`stopPlayback()` reads `sourceRef.current` — which is now the NEW source — so the
previous source's delayed `onended` kills the new clip a few ms in. Every
stop-then-start cycle after the first is killed by the ghost.

**Race 2 — `play()` before `loadAudio()` completes (press-several-times).**
Clicking "Listen" sets `clipPath` → the effect kicks off `loadAudio()` (slow
async chain: AudioContext create/resume → `invoke('read_audio_file')` IPC →
`decodeAudioData`). The play button is immediately clickable; a click before the
buffer lands throws `No audio buffer loaded`. Additionally the AudioContext is
created/resumed by `loadAudio` OUTSIDE a user gesture, so in WebView2 it can
start `suspended` and the first `play()` click throws "invalid state".

**Secondary:** `updateTime` early-stop compares against the `duration` STATE from
the render that created the closure (stale-value hazard); the unmount cleanup
calls `sourceRef.current.stop()` without try/catch (throws if the source already
ended).

## Changes

### A. `frontend/src/hooks/useAudioPlayer.ts`

1. **Guard `onended` against stale sources.** In `play()`, capture the source in
   the closure and only act if it is still the current one:
   ```ts
   const src = sourceRef.current;
   src.onended = () => {
     if (sourceRef.current === src) {
       stopPlayback();
       setCurrentTime(0);
       seekTimeRef.current = 0;
     }
   };
   ```
   (The `sourceRef.current === src` check makes a delayed `onended` from a
   previous source a no-op. This alone fixes the burst-then-stop.)

2. **Make `play()` await the buffer.** Add a `loadingRef = useRef(false)` and a
   `ready` state (see 5). At the top of `play()`, before the buffer check, if
   `!audioBufferRef.current` then `await loadAudio()` first, guarded so concurrent
   clicks don't double-load:
   ```ts
   if (!audioBufferRef.current) {
     if (loadingRef.current) { return; }
     loadingRef.current = true;
     try { await loadAudio(); } finally { loadingRef.current = false; }
     if (!audioBufferRef.current) {
       throw new Error('No audio buffer loaded');
     }
   }
   ```
   Also make `loadAudio()` set `loadingRef` the same way (or have `play()` be the
   only guard — pick one owner, don't double-guard the same flag in both paths
   in a way that deadlocks). After `await loadAudio()` in `play()`, re-run the
   `initAudioContext`/state checks that already follow (the existing
   `state !== 'running'` check stays — a second click will resume it if needed).

3. **Cap `updateTime` on the buffer's duration (ref, not state).** Replace
   `if (newTime >= duration)` with
   `if (newTime >= (audioBufferRef.current?.duration ?? duration))`. Leave
   `onended` as the authority for natural end-of-playback.

4. **try/catch the unmount cleanup `stop()`:**
   ```ts
   if (sourceRef.current) {
     try { sourceRef.current.stop(); } catch (e) { /* already ended */ }
   }
   ```

5. **Expose a `ready` state** in the returned object:
   `ready` = `audioBufferRef.current !== null && !error`. Set it via
   `setReady(true)` at the end of a successful `loadAudio()`, `setReady(false)`
   when `audioPath` changes to a new/other path (top of the `useEffect` that
   calls `loadAudio()`) and on error. (A ref + state pair is fine; keep the ref
   as source of truth for `play()`'s guard, the state for the UI.)

### B. `frontend/src/components/MeetingDetails/SpeakerDiarization.tsx` (clip player UI)

In the clip-player block (the `clipPath && clipName` div, ~line 461):
- Gate the play/pause button: `disabled={!player.ready && !player.isPlaying}`.
- While `!player.ready && clipPath`, show a small loading indicator in the
  "Listening: {clipName}" row (e.g. a `Loader2 className="w-4 h-4 animate-spin"`
  before the text, or the text "Loading clip…" instead of the name).
- Do NOT change the "Listen" button behavior: it only loads (`handleListen` →
  `setClipPath`); it must never trigger playback.

## Invariants (do NOT change)
- No autoplay: playback starts ONLY on an explicit play-button click. Loading a
  clip (including switching speaker) must never call `play()`.
- No Rust changes, no `Cargo.toml` changes, no layout.tsx changes.
- Keep the existing `read_audio_file` IPC path (do not switch to <audio> tags or
  blob URLs).

## Verification (run on this box)
1. `cd frontend && npx tsc --noEmit` — clean.
2. Grep `useAudioPlayer.ts`: `sourceRef.current === src` present in the onended
   handler; `loadingRef` present; `ready` in the returned object; cleanup `stop()`
   inside a try/catch.
3. Grep `SpeakerDiarization.tsx`: the play button has a `disabled` gate on
   `player.ready`.
4. Confirm `git diff --stat` shows ONLY `useAudioPlayer.ts` +
   `SpeakerDiarization.tsx` (no Rust, no layout.tsx).
