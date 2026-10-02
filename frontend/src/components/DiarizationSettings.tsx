"use client";

import { useState, useEffect, useRef } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { Button } from "@/components/ui/button";
import { Label } from "@/components/ui/label";
import { toast } from "sonner";

type ModelStatus = "unknown" | "not_downloaded" | "downloading" | "ready" | "error";

export function DiarizationSettings() {
    const [status, setStatus] = useState<ModelStatus>("unknown");
    const [progress, setProgress] = useState(0);
    const [error, setError] = useState<string | null>(null);
    const listeningRef = useRef(false);

    // Load model status on mount.
    useEffect(() => {
        checkModelStatus();
    }, []);

    const checkModelStatus = async () => {
        try {
            const ready = await invoke<boolean>("is_diarization_model_downloaded");
            setStatus(ready ? "ready" : "not_downloaded");
        } catch (e) {
            console.error("Failed to check diarization model status:", e);
            setStatus("unknown");
        }
    };

    // Listen for download progress / complete / error events.
    useEffect(() => {
        let unlistenProgress: (() => void) | null = null;
        let unlistenComplete: (() => void) | null = null;
        let unlistenError: (() => void) | null = null;

        const setup = async () => {
            if (listeningRef.current) return;
            listeningRef.current = true;

            unlistenProgress = await listen<{ percent: number }>(
                "diarization-model-download-progress",
                (event) => {
                    setStatus("downloading");
                    setProgress(event.payload.percent ?? 0);
                }
            );
            unlistenComplete = await listen("diarization-model-download-complete", () => {
                setProgress(100);
                setStatus("ready");
                toast.success("Diarization model ready", {
                    description: "You can now assign speakers to your meetings",
                });
            });
            unlistenError = await listen<{ error: string }>(
                "diarization-model-download-error",
                (event) => {
                    setStatus("error");
                    setError(event.payload.error ?? "Download failed");
                }
            );
        };

        setup();
        return () => {
            unlistenProgress?.();
            unlistenComplete?.();
            unlistenError?.();
            listeningRef.current = false;
        };
    }, []);

    const handleDownload = async () => {
        setError(null);
        setStatus("downloading");
        setProgress(0);
        try {
            await invoke("download_diarization_model");
            // Completion is handled via the event listener.
        } catch (e) {
            setStatus("error");
            setError(e instanceof Error ? e.message : "Download failed");
            toast.error("Failed to download diarization model", {
                description: e instanceof Error ? e.message : "Unknown error",
            });
        }
    };

    const isDownloading = status === "downloading";

    return (
        <div className="rounded-lg border border-border bg-card p-4 space-y-4">
            <div>
                <Label className="text-sm font-medium text-foreground">
                    Speaker Diarization
                </Label>
                <p className="text-xs text-muted-foreground mt-1">
                    Identify who said what, after a meeting is recorded. Runs locally.
                </p>
            </div>

            <div className="space-y-2">
                <div className="flex items-center justify-between">
                    <span className="text-sm text-muted-foreground">
                        {status === "ready"
                            ? "Model ready"
                            : status === "downloading"
                              ? "Downloading model…"
                              : status === "error"
                                ? "Download failed"
                                : "Model not downloaded"}
                    </span>
                    {status === "ready" ? (
                        <span className="flex items-center gap-1.5 text-xs text-green-600">
                            <span className="w-2 h-2 bg-green-500 rounded-full" />
                            Ready
                        </span>
                    ) : (
                        <Button
                            size="sm"
                            variant="outline"
                            onClick={handleDownload}
                            disabled={isDownloading}
                        >
                            {status === "error" ? "Retry download" : "Download model"}
                        </Button>
                    )}
                </div>

                {isDownloading && (
                    <div className="space-y-1">
                        <div className="w-full h-2 bg-muted rounded-full overflow-hidden">
                            <div
                                className="h-full bg-blue-600 rounded-full transition-all duration-300 ease-out"
                                style={{ width: `${Math.min(progress, 100)}%` }}
                            />
                        </div>
                        <p className="text-xs text-muted-foreground">
                            {Math.round(progress)}% · ~102 MB (Nemotron-3-Diarization)
                        </p>
                    </div>
                )}

                {status === "error" && error && (
                    <p className="text-xs text-red-600">{error}</p>
                )}
            </div>
        </div>
    );
}
