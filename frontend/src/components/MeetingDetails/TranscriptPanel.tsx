"use client";

import { Transcript, TranscriptSegmentData } from '@/types';
import { TranscriptView } from '@/components/TranscriptView';
import { VirtualizedTranscriptView } from '@/components/VirtualizedTranscriptView';
import { TranscriptButtonGroup } from './TranscriptButtonGroup';
import { useMemo, type ReactNode } from 'react';

interface TranscriptPanelProps {
  transcripts: Transcript[];
  customPrompt: string;
  onPromptChange: (value: string) => void;
  onCopyTranscript: () => void;
  onOpenMeetingFolder: () => Promise<void>;
  isRecording: boolean;
  disableAutoScroll?: boolean;

  // Optional pagination props (when using virtualization)
  usePagination?: boolean;
  segments?: TranscriptSegmentData[];
  hasMore?: boolean;
  isLoadingMore?: boolean;
  totalCount?: number;
  loadedCount?: number;
  onLoadMore?: () => void;

  // Retranscription props
  meetingId?: string;
  meetingFolderPath?: string | null;
  onRefetchTranscripts?: () => Promise<void>;

  // Speaker diarization (post-processing)
  speakerNameMap?: Record<number, string>;
  speakerControl?: ReactNode;
}

export function TranscriptPanel({
  transcripts,
  customPrompt,
  onPromptChange,
  onCopyTranscript,
  onOpenMeetingFolder,
  isRecording,
  disableAutoScroll = false,
  usePagination = false,
  segments,
  hasMore,
  isLoadingMore,
  totalCount,
  loadedCount,
  onLoadMore,
  meetingId,
  meetingFolderPath,
  onRefetchTranscripts,
  speakerNameMap,
  speakerControl,
}: TranscriptPanelProps) {
  // Convert transcripts to segments if pagination is not used but we want virtualization
  const convertedSegments = useMemo(() => {
    let base: TranscriptSegmentData[];
    if (usePagination && segments) {
      base = segments;
    } else {
      // Convert transcripts to segments for virtualization
      base = transcripts.map(t => ({
        id: t.id,
        timestamp: t.audio_start_time ?? 0,
        endTime: t.audio_end_time,
        text: t.text,
        confidence: t.confidence,
        speaker_id: t.speaker_id,
        speaker_review: t.speaker_review,
      }));
    }
    // Resolve the display name for each segment's speaker.
    if (!speakerNameMap) return base;
    return base.map(s => ({
      ...s,
      speaker_name: s.speaker_id !== undefined ? speakerNameMap[s.speaker_id] : undefined,
    }));
  }, [transcripts, usePagination, segments, speakerNameMap]);

  return (
    <div className="flex h-full min-w-0 w-full bg-card flex-col relative @container">
      {/* Title area */}
      <div className="p-4 border-b border-border space-y-2">
        <TranscriptButtonGroup
          transcriptCount={usePagination ? (totalCount ?? convertedSegments.length) : (transcripts?.length || 0)}
          onCopyTranscript={onCopyTranscript}
          onOpenMeetingFolder={onOpenMeetingFolder}
          meetingId={meetingId}
          meetingFolderPath={meetingFolderPath}
          onRefetchTranscripts={onRefetchTranscripts}
        />
        {speakerControl}
      </div>

      {/* Transcript content - use virtualized view for better performance */}
      <div className="flex-1 overflow-hidden pb-4">
        <VirtualizedTranscriptView
          segments={convertedSegments}
          isRecording={isRecording}
          isPaused={false}
          isProcessing={false}
          isStopping={false}
          enableStreaming={false}
          showConfidence={true}
          disableAutoScroll={disableAutoScroll}
          hasMore={hasMore}
          isLoadingMore={isLoadingMore}
          totalCount={totalCount}
          loadedCount={loadedCount}
          onLoadMore={onLoadMore}
        />
      </div>

      {/* Custom prompt input at bottom of transcript section */}
      {!isRecording && convertedSegments.length > 0 && (
        <div className="p-1 border-t border-border">
          <textarea
            placeholder="Add context for AI summary. For example people involved, meeting overview, objective etc..."
            className="w-full px-3 py-2 border border-border rounded-md text-sm focus:outline-none focus:ring-1 focus:ring-blue-500 focus:border-blue-500 bg-background shadow-sm min-h-[80px] resize-y"
            value={customPrompt}
            onChange={(e) => onPromptChange(e.target.value)}
          />
        </div>
      )}
    </div>
  );
}
