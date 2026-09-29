import React, { useState, useEffect } from "react";
import { getVersion } from '@tauri-apps/api/app';
import Image from 'next/image';
import { UpdateDialog } from "./UpdateDialog";
import { updateService, UpdateInfo } from '@/services/updateService';
import { Button } from './ui/button';
import { Loader2, CheckCircle2 } from 'lucide-react';
import { toast } from 'sonner';


export function About() {
    const [currentVersion, setCurrentVersion] = useState<string>('0.4.1');
    const [updateInfo, setUpdateInfo] = useState<UpdateInfo | null>(null);
    const [isChecking, setIsChecking] = useState(false);
    const [showUpdateDialog, setShowUpdateDialog] = useState(false);

    useEffect(() => {
        // Get current version on mount
        getVersion().then(setCurrentVersion).catch(console.error);
    }, []);

    const handleCheckForUpdates = async () => {
        setIsChecking(true);
        try {
            const info = await updateService.checkForUpdates(true);
            setUpdateInfo(info);
            if (info.available) {
                setShowUpdateDialog(true);
            } else {
                toast.success('You are running the latest version');
            }
        } catch (error: any) {
            console.error('Failed to check for updates:', error);
            toast.error('Failed to check for updates: ' + (error.message || 'Unknown error'));
        } finally {
            setIsChecking(false);
        }
    };

    return (
        <div className="p-4 space-y-4 h-[80vh] overflow-y-auto">
            {/* Compact Header */}
            <div className="text-center">
                <div className="mb-3">
                    <Image
                        src="icon_128x128.png"
                        alt="Minutely Logo"
                        width={64}
                        height={64}
                        className="mx-auto"
                    />
                </div>
                {/* <h1 className="text-xl font-bold text-gray-900">Meetily</h1> */}
                <span className="text-sm text-muted-foreground"> v{currentVersion}</span>
                <p className="text-medium text-muted-foreground mt-1">
                    Local-first meeting notes & summaries — with the Pro features, minus the Pro price.
                </p>
                <div className="mt-3">
                    <Button
                        onClick={handleCheckForUpdates}
                        disabled={isChecking}
                        variant="outline"
                        size="sm"
                        className="text-xs"
                    >
                        {isChecking ? (
                            <>
                                <Loader2 className="h-3 w-3 mr-2 animate-spin" />
                                Checking...
                            </>
                        ) : (
                            <>
                                <CheckCircle2 className="h-3 w-3 mr-2" />
                                Check for Updates
                            </>
                        )}
                    </Button>
                    {updateInfo?.available && (
                        <div className="mt-2 text-xs text-blue-600">
                            Update available: v{updateInfo.version}
                        </div>
                    )}
                </div>
            </div>

            {/* Features Grid - Compact */}
            <div className="space-y-3">
                <h2 className="text-base font-semibold text-foreground">What makes Minutely different</h2>
                <div className="grid grid-cols-3 gap-2">
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Privacy-first</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Your data & AI processing workflow can now stay within your premise. No cloud, no leaks.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Use Any Model</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Prefer local open-source model? Great. Want to plug in an external API? Also fine. No lock-in.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Cost-Smart</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Avoid pay-per-minute bills by running models locally (or pay only for the calls you choose).</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Works everywhere</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Google Meet, Zoom, Teams-online or offline.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Speaker Diarization</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Know who said what — on-device, no Pro tier required.</p>
                    </div>
                    <div className="bg-muted rounded p-3 hover:bg-accent transition-colors">
                        <h3 className="font-bold text-sm text-foreground mb-1">Dark mode</h3>
                        <p className="text-xs text-muted-foreground leading-relaxed">Late-night meetings? Your eyes, too. Toggle it in settings.</p>
                    </div>
                </div>
            </div>

            {/* Roadmap - Compact */}
            <div className="bg-blue-50 dark:bg-blue-950 rounded p-3">
                <p className="text-s text-blue-800 dark:text-blue-300">
                    <span className="font-bold">On the fork's roadmap:</span> speaker diarization, advanced exports, and custom summary templates — the Pro feature set, self-hosted.
                </p>
            </div>

            {/* Footer - Compact */}
            <div className="pt-2 border-t border-border text-center">
                <p className="text-xs text-muted-foreground">
                    Minutely — a personal fork, based on{' '}
                    <a
                        href="https://github.com/Zackriya-Solutions/meetily"
                        target="_blank"
                        rel="noopener noreferrer"
                        className="underline hover:text-foreground font-medium"
                    >
                        Meetily
                    </a>{' '}
                    (MIT)
                </p>
                <p className="text-xs text-muted-foreground mt-1">
                    <a
                        href="https://github.com/fizzmail/meetily"
                        target="_blank"
                        rel="noopener noreferrer"
                        className="underline hover:text-foreground font-medium"
                    >
                        github.com/fizzmail/meetily
                    </a>
                </p>
            </div>

            {/* Update Dialog */}
            <UpdateDialog
                open={showUpdateDialog}
                onOpenChange={setShowUpdateDialog}
                updateInfo={updateInfo}
            />
        </div>

    )
}