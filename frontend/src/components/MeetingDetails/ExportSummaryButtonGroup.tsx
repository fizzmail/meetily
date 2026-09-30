"use client";

import { invoke } from '@tauri-apps/api/core';
import { Button } from '@/components/ui/button';
import { ButtonGroup } from '@/components/ui/button-group';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';
import { Download, Loader2, FileText, File } from 'lucide-react';
import { toast } from 'sonner';

type ExportFormat = 'markdown' | 'pdf' | 'docx';

interface ExportSummaryButtonGroupProps {
  meetingId: string;
  meetingName: string;
  summaryData: unknown;
  hasSummary: boolean;
  summaryStatus: 'idle' | 'processing' | 'summarizing' | 'regenerating' | 'completed' | 'error';
}

const EXPORT_COMMANDS: Record<ExportFormat, string> = {
  markdown: 'api_export_summary_markdown',
  pdf: 'api_export_summary_pdf',
  docx: 'api_export_summary_docx',
};

const EXPORT_LABELS: Record<ExportFormat, string> = {
  markdown: 'Markdown (.md)',
  pdf: 'PDF (.pdf)',
  docx: 'Word (.docx)',
};

function isTauriAvailable(): boolean {
  return typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;
}

/**
 * Builds the `content_json` payload for the export commands. Prefers the
 * Blocknote block array when present; otherwise falls back to wrapping the
 * markdown string in a single paragraph block. Returns null when there is
 * nothing to export.
 */
function buildContentJson(summaryData: unknown): string | null {
  if (!summaryData || typeof summaryData !== 'object') return null;
  const data = summaryData as Record<string, unknown>;

  if (Array.isArray(data.summary_json)) {
    return JSON.stringify(data.summary_json);
  }

  if (typeof data.markdown === 'string' && data.markdown.trim().length > 0) {
    return JSON.stringify([{ type: 'paragraph', content: [{ text: data.markdown }] }]);
  }

  return null;
}

function getErrorMessage(err: unknown): string {
  if (typeof err === 'string') return err;
  if (err instanceof Error) return err.message;
  try {
    return JSON.stringify(err);
  } catch {
    return 'Export failed';
  }
}

export function ExportSummaryButtonGroup({
  meetingId,
  meetingName,
  summaryData,
  hasSummary,
  summaryStatus,
}: ExportSummaryButtonGroupProps) {
  const isGenerating =
    summaryStatus === 'processing' ||
    summaryStatus === 'summarizing' ||
    summaryStatus === 'regenerating';
  const isDisabled = !hasSummary || isGenerating;

  const handleExport = async (format: ExportFormat) => {
    // Browser preview guard: never call invoke outside the Tauri desktop app.
    if (!isTauriAvailable()) {
      toast.info(
        'Export requires the desktop app (Tauri). In the browser preview, this is a layout preview only.'
      );
      return;
    }

    const contentJson = buildContentJson(summaryData);
    if (contentJson === null) {
      toast.error('Could not export summary', {
        description: 'No summary content is available to export.',
      });
      return;
    }

    try {
      const savedPath = await invoke<string>(EXPORT_COMMANDS[format], {
        meetingId,
        meetingName,
        contentJson,
      });
      toast.success('Summary exported', { description: savedPath });
    } catch (err) {
      toast.error('Export failed', { description: getErrorMessage(err) });
    }
  };

  return (
    <ButtonGroup>
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <Button
            variant="outline"
            size="sm"
            title={isGenerating ? 'Generating summary...' : 'Export summary'}
            aria-label="Export summary"
            disabled={isDisabled}
          >
            {isGenerating ? (
              <>
                <Loader2 className="animate-spin" />
                <span className="hidden @[40rem]:inline">Exporting...</span>
              </>
            ) : (
              <>
                <Download />
                <span className="hidden @[40rem]:inline">Export</span>
              </>
            )}
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          {(Object.keys(EXPORT_COMMANDS) as ExportFormat[]).map((format) => (
            <DropdownMenuItem key={format} onClick={() => void handleExport(format)}>
              {format === 'pdf' ? <File /> : <FileText />}
              <span>{EXPORT_LABELS[format]}</span>
            </DropdownMenuItem>
          ))}
        </DropdownMenuContent>
      </DropdownMenu>
    </ButtonGroup>
  );
}
