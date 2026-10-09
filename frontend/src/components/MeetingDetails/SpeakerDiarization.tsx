"use client";

import { useState, useEffect, useCallback, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, UnlistenFn } from "@tauri-apps/api/event";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
    Dialog,
    DialogContent,
    DialogHeader,
    DialogTitle,
    DialogDescription,
} from "@/components/ui/dialog";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { Mic, Play, Pause, Loader2, Users } from "lucide-react";
import { useAudioPlayer } from "@/hooks/useAudioPlayer";
import { MeetingSpeaker } from "@/types";

const PALETTE = [
    { dot: "bg-blue-500", text: "text-blue-600" },
    { dot: "bg-emerald-500", text: "text-emerald-600" },
    { dot: "bg-purple-500", text: "text-purple-600" },
    { dot: "bg-amber-500", text: "text-amber-600" },
    { dot: "bg-rose-500", text: "text-rose-600" },
    { dot: "bg-cyan-500", text: "text-cyan-600" },
    { dot: "bg-lime-500", text: "text-lime-600" },
    { dot: "bg-indigo-500", text: "text-indigo-600" },
];

function colorFor(id: number) {
    return PALETTE[((id % PALETTE.length) + PALETTE.length) % PALETTE.length];
}

type Phase = "preparing" | "analyzing" | "finalizing";

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

interface SpeakerDiarizationControlProps {
    meetingId: string;
    onSpeakersChange: (nameMap: Record<number, string>) => void;
    onRefetchTranscripts?: () => Promise<void>;
}

export function SpeakerDiarizationControl({
    meetingId,
    onSpeakersChange,
    onRefetchTranscripts,
}: SpeakerDiarizationControlProps) {
    const [modelReady, setModelReady] = useState(false);
    const [speakers, setSpeakers] = useState<MeetingSpeaker[]>([]);
    const [isRunning, setIsRunning] = useState(false);
    const [dialogOpen, setDialogOpen] = useState(false);
    const [phase, setPhase] = useState<Phase>("preparing");
    const [elapsedSec, setElapsedSec] = useState(0);
    const startedAtRef = useRef<number | null>(null);

    // Rename
    const [renamingId, setRenamingId] = useState<number | null>(null);
    const [renameValue, setRenameValue] = useState("");

    // Merge
    const [mergingId, setMergingId] = useState<number | null>(null);
    const [mergeTarget, setMergeTarget] = useState<number | "">("");

    // Listen
    const [clipPath, setClipPath] = useState<string | null>(null);
    const [clipSpeaker, setClipSpeaker] = useState<number | null>(null);
    const [loadingClip, setLoadingClip] = useState(false);
    const player = useAudioPlayer(clipPath);

    const checkModel = useCallback(async () => {
        try {
            setModelReady(await invoke<boolean>("is_diarization_model_downloaded"));
        } catch (e) {
            console.error("Failed to check diarization model:", e);
            setModelReady(false);
        }
    }, []);

    const loadSpeakers = useCallback(async () => {
        try {
            const list = await invoke<MeetingSpeaker[]>("get_meeting_speakers", { meetingId });
            const active = list.filter((s) => s.is_merged === 0);
            setSpeakers(active);
            const nameMap: Record<number, string> = {};
            for (const s of active) {
                nameMap[s.speaker_id] = s.display_name || `Speaker ${s.speaker_id}`;
            }
            onSpeakersChange(nameMap);
        } catch (e) {
            console.error("Failed to load meeting speakers:", e);
        }
    }, [meetingId, onSpeakersChange]);

    useEffect(() => {
        checkModel();
        loadSpeakers();
    }, [checkModel, loadSpeakers]);

    useEffect(() => {
        let unlisten: UnlistenFn | null = null;
        listen<{ phase: Phase }>("diarization-run-phase", (e) => {
            if (e.payload && e.payload.phase) setPhase(e.payload.phase);
        }).then((u) => { unlisten = u; });
        return () => { unlisten?.(); };
    }, []);

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

    const publishNames = (list: MeetingSpeaker[]) => {
        const nameMap: Record<number, string> = {};
        for (const s of list) {
            nameMap[s.speaker_id] = s.display_name || `Speaker ${s.speaker_id}`;
        }
        onSpeakersChange(nameMap);
    };

    const runDiarization = async () => {
        setIsRunning(true);
        setDialogOpen(true);
        try {
            const result = await invoke<MeetingSpeaker[]>("run_diarization", { meetingId });
            setSpeakers(result);
            publishNames(result);
            toast.success(`Detected ${result.length} speaker${result.length === 1 ? "" : "s"}`, {
                description: "Name them below to update the transcript",
            });
            if (onRefetchTranscripts) { await onRefetchTranscripts(); }
        } catch (e) {
            // Tauri's invoke() rejects a failed command with the error in one of
            // three shapes depending on the path: a plain string (Result<T,String>),
            // an Error instance, or a plain object with the message in `message` or
            // `error`. Handle all three so a failure (ffmpeg / sidecar stderr) is
            // always diagnosable instead of collapsing to "Unknown error".
            let msg: string;
            if (typeof e === "string" && e.length > 0) {
                msg = e;
            } else if (e instanceof Error && e.message) {
                msg = e.message;
            } else if (e && typeof e === "object") {
                const o = e as Record<string, unknown>;
                msg = (typeof o.message === "string" && o.message) ||
                      (typeof o.error === "string" && o.error) ||
                      (typeof o.code === "string" && o.code) ||
                      JSON.stringify(e);
            } else {
                msg = String(e);
            }
            toast.error("Speaker assignment failed", { description: msg });
        } finally {
            setIsRunning(false);
        }
    };

    const handleRename = async (id: number) => {
        const name = renameValue.trim();
        if (!name) return;
        try {
            await invoke("rename_speaker", { meetingId, speakerId: id, newName: name });
            const updated = speakers.map((s) =>
                s.speaker_id === id ? { ...s, display_name: name } : s
            );
            setSpeakers(updated);
            publishNames(updated);
            setRenamingId(null);
            toast.success("Speaker renamed");
        } catch (e) {
            toast.error("Rename failed", {
                description: e instanceof Error ? e.message : "Unknown error",
            });
        }
    };

    const handleMerge = async (from: number, to: number) => {
        try {
            await invoke("merge_speakers", { meetingId, fromId: from, toId: to });
            // Remove the merged speaker, keep the target.
            const updated = speakers.filter((s) => s.speaker_id !== from);
            setSpeakers(updated);
            publishNames(updated);
            setMergingId(null);
            setMergeTarget("");
            toast.success("Speakers merged");
            if (onRefetchTranscripts) { await onRefetchTranscripts(); }
        } catch (e) {
            toast.error("Merge failed", {
                description: e instanceof Error ? e.message : "Unknown error",
            });
        }
    };

    const handleListen = async (id: number) => {
        setLoadingClip(true);
        try {
            const path = await invoke<string>("get_speaker_clip", {
                meetingId,
                speakerId: id,
            });
            setClipSpeaker(id);
            setClipPath(path);
        } catch (e) {
            toast.error("Could not load a clip", {
                description: e instanceof Error ? e.message : "No audio for this speaker",
            });
        } finally {
            setLoadingClip(false);
        }
    };

    const clipName =
        clipSpeaker !== null
            ? (speakers.find((s) => s.speaker_id === clipSpeaker)?.display_name ||
              `Speaker ${clipSpeaker}`)
            : null;

    return (
        <>
            <Button
                variant="outline"
                size="sm"
                className="px-2 @[22rem]:px-3"
                onClick={runDiarization}
                disabled={!modelReady || isRunning}
                title={
                    speakers.length > 0
                        ? "Re-analyze this meeting's speakers"
                        : modelReady
                        ? "Assign speakers to the transcript"
                        : "Download the diarization model in Settings first"
                }
            >
                <Mic />
                <span className="hidden @[22rem]:inline">
                    {speakers.length > 0 ? "Re-Assign Speakers" : "Assign Speakers"}
                </span>
            </Button>

            {speakers.length > 0 && (
                <Button
                    variant="outline"
                    size="sm"
                    className="px-2"
                    onClick={() => setDialogOpen(true)}
                    title="Manage speakers"
                >
                    <Users />
                    <span className="hidden @[22rem]:inline">
                        Speakers ({speakers.length})
                    </span>
                </Button>
            )}

            <Dialog open={dialogOpen} onOpenChange={setDialogOpen}>
                <DialogContent className="max-w-lg max-h-[80vh] overflow-y-auto">
                    <DialogHeader>
                        <DialogTitle>Speaker Diarization</DialogTitle>
                        <DialogDescription>
                            Identify and name the people in this meeting.
                        </DialogDescription>
                    </DialogHeader>

                    {isRunning ? (
                        <div className="flex flex-col items-center justify-center py-10 gap-3">
                            <Loader2 className="w-8 h-8 animate-spin text-blue-600" />
                            <p className="text-sm text-muted-foreground">{PHASE_LABEL[phase]}</p>
                            <p className="text-xs text-muted-foreground tabular-nums">{formatElapsed(elapsedSec)}</p>
                            {speakers.length > 0 && (
                                <p className="text-xs text-muted-foreground">
                                    Re-run — re-analyzing the whole meeting
                                </p>
                            )}
                        </div>
                    ) : speakers.length === 0 ? (
                        <div className="py-6 text-center space-y-3">
                            <p className="text-sm text-muted-foreground">
                                No speakers assigned yet.
                            </p>
                            <Button
                                onClick={runDiarization}
                                disabled={!modelReady}
                                title={
                                    modelReady
                                        ? "Run speaker assignment"
                                        : "Download the diarization model in Settings first"
                                }
                            >
                                <Mic />
                                Assign Speakers
                            </Button>
                            {!modelReady && (
                                <p className="text-xs text-muted-foreground">
                                    The diarization model is not downloaded yet. Download it in
                                    Settings → Transcription.
                                </p>
                            )}
                        </div>
                    ) : (
                        <div className="space-y-3">
                            {speakers.map((s) => {
                                const color = colorFor(s.speaker_id);
                                const isRenaming = renamingId === s.speaker_id;
                                const isMerging = mergingId === s.speaker_id;
                                return (
                                    <div
                                        key={s.speaker_id}
                                        className="rounded-lg border border-border p-3 space-y-2"
                                    >
                                        <div className="flex items-center gap-2">
                                            <span className={`w-2.5 h-2.5 rounded-full ${color.dot}`} />
                                            {isRenaming ? (
                                                <Input
                                                    autoFocus
                                                    value={renameValue}
                                                    onChange={(e) => setRenameValue(e.target.value)}
                                                    onKeyDown={(e) => {
                                                        if (e.key === "Enter") handleRename(s.speaker_id);
                                                    }}
                                                    className="h-8 flex-1"
                                                    placeholder="Speaker name"
                                                />
                                            ) : (
                                                <span
                                                    className={`text-sm font-medium flex-1 ${color.text}`}
                                                >
                                                    {s.display_name || `Speaker ${s.speaker_id}`}
                                                </span>
                                            )}
                                        </div>

                                        <div className="flex items-center gap-2 flex-wrap">
                                            {isRenaming ? (
                                                <>
                                                    <Button
                                                        size="sm"
                                                        onClick={() => handleRename(s.speaker_id)}
                                                    >
                                                        Save
                                                    </Button>
                                                    <Button
                                                        size="sm"
                                                        variant="ghost"
                                                        onClick={() => setRenamingId(null)}
                                                    >
                                                        Cancel
                                                    </Button>
                                                </>
                                            ) : (
                                                <Button
                                                    size="sm"
                                                    variant="ghost"
                                                    onClick={() => {
                                                        setRenamingId(s.speaker_id);
                                                        setRenameValue(
                                                            s.display_name || `Speaker ${s.speaker_id}`
                                                        );
                                                    }}
                                                >
                                                    Rename
                                                </Button>
                                            )}

                                            <Button
                                                size="sm"
                                                variant="ghost"
                                                onClick={() => handleListen(s.speaker_id)}
                                                disabled={loadingClip}
                                            >
                                                {loadingClip ? (
                                                    <Loader2 className="w-4 h-4 animate-spin" />
                                                ) : (
                                                    <Play className="w-4 h-4" />
                                                )}
                                                Listen
                                            </Button>

                                            {speakers.length > 1 &&
                                                (isMerging ? (
                                                    <div className="flex items-center gap-2">
                                                        <Select
                                                            value={String(mergeTarget)}
                                                            onValueChange={(v) =>
                                                                setMergeTarget(Number(v))
                                                            }
                                                        >
                                                            <SelectTrigger className="h-8 w-32">
                                                                <SelectValue placeholder="Into…" />
                                                            </SelectTrigger>
                                                            <SelectContent>
                                                                {speakers
                                                                    .filter((t) => t.speaker_id !== s.speaker_id)
                                                                    .map((t) => (
                                                                        <SelectItem
                                                                            key={t.speaker_id}
                                                                            value={String(t.speaker_id)}
                                                                        >
                                                                            {t.display_name ||
                                                                                `Speaker ${t.speaker_id}`}
                                                                        </SelectItem>
                                                                    ))}
                                                            </SelectContent>
                                                        </Select>
                                                        <Button
                                                            size="sm"
                                                            onClick={() =>
                                                                mergeTarget !== "" &&
                                                                handleMerge(s.speaker_id, mergeTarget)
                                                            }
                                                            disabled={mergeTarget === ""}
                                                        >
                                                            Merge
                                                        </Button>
                                                        <Button
                                                            size="sm"
                                                            variant="ghost"
                                                            onClick={() => {
                                                                setMergingId(null);
                                                                setMergeTarget("");
                                                            }}
                                                        >
                                                            Cancel
                                                        </Button>
                                                    </div>
                                                ) : (
                                                    <Button
                                                        size="sm"
                                                        variant="ghost"
                                                        onClick={() => setMergingId(s.speaker_id)}
                                                    >
                                                        Merge into…
                                                    </Button>
                                                ))}
                                        </div>
                                    </div>
                                );
                            })}

                            {/* Clip player */}
                            {clipPath && clipName && (
                                <div className="rounded-lg border border-blue-200 dark:border-blue-800 bg-blue-50 dark:bg-blue-950 p-3 flex items-center gap-3">
                                    <Button
                                        size="icon"
                                        onClick={() => {
                                            if (player.isPlaying) player.pause();
                                            else player.play();
                                        }}
                                    >
                                        {player.isPlaying ? (
                                            <Pause className="w-4 h-4" />
                                        ) : (
                                            <Play className="w-4 h-4" />
                                        )}
                                    </Button>
                                    <span className="text-sm text-blue-900 dark:text-blue-300 flex-1">
                                        Listening: {clipName}
                                    </span>
                                    <Button
                                        size="sm"
                                        variant="ghost"
                                        onClick={() => {
                                            player.pause();
                                            setClipPath(null);
                                            setClipSpeaker(null);
                                        }}
                                    >
                                        Stop
                                    </Button>
                                </div>
                            )}
                            {player.error && (
                                <p className="text-xs text-red-600">{player.error}</p>
                            )}
                        </div>
                    )}
                </DialogContent>
            </Dialog>
        </>
    );
}
