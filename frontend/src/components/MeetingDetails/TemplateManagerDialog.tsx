"use client";

import { useState, useEffect } from 'react';
import {
  FilePlus2,
  Trash2,
  Plus,
  X,
  Save,
  Braces,
  Loader2,
} from 'lucide-react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from '@/components/ui/dialog';
import { Button } from '@/components/ui/button';
import { Input } from '@/components/ui/input';
import { Textarea } from '@/components/ui/textarea';
import { Label } from '@/components/ui/label';
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from '@/components/ui/select';
import { ScrollArea } from '@/components/ui/scroll-area';
import { Separator } from '@/components/ui/separator';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import {
  useTemplateManager,
  isTauriAvailable,
  type TemplateEntry,
} from '@/hooks/meeting-details/useTemplateManager';

interface TemplateManagerDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

type SectionFormat = 'paragraph' | 'list' | 'string';

interface SectionDraft {
  id: string;
  title: string;
  instruction: string;
  format: SectionFormat;
  item_format: string;
}

const SECTION_FORMATS: SectionFormat[] = ['paragraph', 'list', 'string'];

// Representative sections for the built-in templates so the editor layout is
// visible in the browser preview (no Tauri backend to fetch the JSON from).
const PREVIEW_BUILTIN_SECTIONS: Record<string, Omit<SectionDraft, 'id'>[]> = {
  standard_meeting: [
    {
      title: 'Summary',
      instruction: 'Provide a brief, one-paragraph executive summary of the entire meeting.',
      format: 'paragraph',
      item_format: '',
    },
    {
      title: 'Key Decisions',
      instruction: 'List the most important decisions made during the meeting.',
      format: 'list',
      item_format: '',
    },
    {
      title: 'Action Items',
      instruction:
        'List all assigned tasks with their owners and due date. Always add reference transcript segment and timestamp in the table.',
      format: 'list',
      item_format:
        '| **Owner** | Task | Due | Reference Transcript Segment | Segment Time stamp |\n| --- | --- | --- | --- | --- |',
    },
    {
      title: 'Discussion Highlights',
      instruction: 'Summarize the main topics of discussion, key arguments, and important insights.',
      format: 'paragraph',
      item_format: '',
    },
  ],
  daily_standup: [
    { title: 'Date', instruction: 'YYYY-MM-DD', format: 'string', item_format: '' },
    { title: 'Attendees', instruction: 'List of participants present', format: 'list', item_format: '' },
    {
      title: 'Yesterday',
      instruction: 'What I completed yesterday (short bullets)',
      format: 'list',
      item_format: '| **Owner** | **Completed Work** |\n| --- | --- |',
    },
    {
      title: 'Today',
      instruction: 'Planned work for today (short bullets)',
      format: 'list',
      item_format: '| **Owner** | **Planned Work** |\n| --- | --- |',
    },
    {
      title: 'Blockers',
      instruction: 'Any impediments and owner if known',
      format: 'list',
      item_format: '| **Owner** | **Blocker** | Impact |\n| --- | --- | --- |',
    },
    { title: 'Notes', instruction: 'Optional quick notes or announcements', format: 'paragraph', item_format: '' },
  ],
};

// crypto.randomUUID() is only available in secure contexts (https/localhost).
// On a plain LAN IP (the browser preview) it is undefined, so fall back to a
// timestamp+counter id to avoid throwing during render.
let idCounter = 0;
function genId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  idCounter += 1;
  return `section_${Date.now()}_${idCounter}`;
}

function newSectionDraft(): SectionDraft {
  return {
    id: genId(),
    title: '',
    instruction: '',
    format: 'paragraph',
    item_format: '',
  };
}

function sectionsFromPreviewBuiltin(templateId: string): SectionDraft[] {
  const source = PREVIEW_BUILTIN_SECTIONS[templateId];
  if (!source) return [newSectionDraft()];
  return source.map((section) => ({ ...section, id: genId() }));
}

function buildTemplateJson(name: string, description: string, sections: SectionDraft[]): string {
  const payload = {
    name,
    description,
    sections: sections.map((section) => {
      const json: Record<string, string> = {
        title: section.title,
        instruction: section.instruction,
        format: section.format,
      };
      if (section.format === 'list' && section.item_format.trim() !== '') {
        json.item_format = section.item_format;
      }
      return json;
    }),
  };
  return JSON.stringify(payload, null, 2);
}

function validateTemplate(
  name: string,
  description: string,
  sections: SectionDraft[]
): string | null {
  if (!name.trim()) return 'Template name is required.';
  if (!description.trim()) return 'Template description is required.';
  if (sections.length === 0) return 'Template must have at least one section.';
  for (let i = 0; i < sections.length; i++) {
    const section = sections[i];
    if (!section.title.trim()) return `Section ${i + 1} is missing a title.`;
    if (!section.instruction.trim()) {
      return `Section "${section.title.trim()}" is missing an instruction.`;
    }
    if (!SECTION_FORMATS.includes(section.format)) {
      return `Section "${section.title.trim()}" has an invalid format.`;
    }
  }
  return null;
}

export function TemplateManagerDialog({ open, onOpenChange }: TemplateManagerDialogProps) {
  const { templates, refresh, loading, error } = useTemplateManager();
  const tauriAvailable = isTauriAvailable();
  const isDisabled = !tauriAvailable;

  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [sections, setSections] = useState<SectionDraft[]>([newSectionDraft()]);
  const [fetchedJson, setFetchedJson] = useState<string | null>(null);
  const [showRawJson, setShowRawJson] = useState(false);
  const [saving, setSaving] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [validationError, setValidationError] = useState<string | null>(null);

  const selectedTemplate: TemplateEntry | null =
    templates.find((template) => template.id === selectedId) ?? null;
  const isCustomSelected = selectedTemplate?.is_custom === true;

  // Reset the editor to a blank "new template" state on each open
  useEffect(() => {
    if (open) {
      setSelectedId(null);
      setName('');
      setDescription('');
      setSections([newSectionDraft()]);
      setFetchedJson(null);
      setShowRawJson(false);
      setConfirmDelete(false);
      setValidationError(null);
    }
  }, [open]);

  const loadTemplateIntoEditor = async (template: TemplateEntry) => {
    setSelectedId(template.id);
    setName(template.name);
    setDescription(template.description);
    setConfirmDelete(false);
    setValidationError(null);

    if (!tauriAvailable) {
      // Browser preview: no backend to fetch from; use representative sections
      setSections(sectionsFromPreviewBuiltin(template.id));
      setFetchedJson(null);
      return;
    }

    try {
      const json = await invoke<string>('api_get_template_json', {
        templateId: template.id,
      });
      setFetchedJson(json);
      try {
        const parsed = JSON.parse(json) as {
          sections?: Array<{
            title?: unknown;
            instruction?: unknown;
            format?: unknown;
            item_format?: unknown;
          }>;
        };
        const parsedSections = Array.isArray(parsed.sections) ? parsed.sections : [];
        setSections(
          parsedSections.map(
            (section): SectionDraft => ({
              id: genId(),
              title: typeof section.title === 'string' ? section.title : '',
              instruction: typeof section.instruction === 'string' ? section.instruction : '',
              format: SECTION_FORMATS.includes(section.format as SectionFormat)
                ? (section.format as SectionFormat)
                : 'paragraph',
              item_format: typeof section.item_format === 'string' ? section.item_format : '',
            })
          )
        );
      } catch {
        setSections([newSectionDraft()]);
      }
    } catch (err) {
      console.error('Failed to load template JSON:', err);
      setFetchedJson(null);
      setSections([newSectionDraft()]);
    }
  };

  const handleNewTemplate = () => {
    setSelectedId(null);
    setName('');
    setDescription('');
    setSections([newSectionDraft()]);
    setFetchedJson(null);
    setConfirmDelete(false);
    setValidationError(null);
  };

  const updateSection = (sectionId: string, patch: Partial<Omit<SectionDraft, 'id'>>) => {
    setSections((prev) =>
      prev.map((section) => (section.id === sectionId ? { ...section, ...patch } : section))
    );
  };

  const addSection = () => {
    setSections((prev) => [...prev, newSectionDraft()]);
    setValidationError(null);
  };

  const removeSection = (sectionId: string) => {
    setSections((prev) => prev.filter((section) => section.id !== sectionId));
    setValidationError(null);
  };

  const clientJson = buildTemplateJson(name, description, sections);
  // For a saved template this is the JSON fetched on select; for a new
  // unsaved template (or the browser preview) it is the JSON that would be saved.
  const rawJsonPreview = fetchedJson ?? clientJson;

  const handleSave = async () => {
    if (isDisabled) return;
    const problem = validateTemplate(name, description, sections);
    if (problem) {
      setValidationError(problem);
      return;
    }
    setValidationError(null);
    setSaving(true);
    try {
      const savedId = await invoke<string>('api_save_template', {
        templateJson: clientJson,
      });
      await refresh();
      setSelectedId(savedId);
      setConfirmDelete(false);
      toast.success('Template saved', {
        description: `"${name.trim()}" is now available for summaries.`,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setValidationError(`Failed to save template: ${message}`);
      toast.error('Failed to save template', { description: message });
    } finally {
      setSaving(false);
    }
  };

  const handleDelete = async () => {
    if (!selectedTemplate || !selectedTemplate.is_custom) return;
    setDeleting(true);
    try {
      await invoke('api_delete_template', { templateId: selectedTemplate.id });
      await refresh();
      setSelectedId(null);
      setName('');
      setDescription('');
      setSections([newSectionDraft()]);
      setFetchedJson(null);
      setConfirmDelete(false);
      toast.success('Template deleted', {
        description: `"${selectedTemplate.name}" was removed.`,
      });
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err);
      setValidationError(`Failed to delete template: ${message}`);
      toast.error('Failed to delete template', { description: message });
    } finally {
      setDeleting(false);
    }
  };

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <FilePlus2 className="h-5 w-5" />
            Manage Templates
          </DialogTitle>
          <DialogDescription>
            Create, edit, and delete the templates used for AI summary generation.
          </DialogDescription>
        </DialogHeader>

        {error && (
          <Alert variant={tauriAvailable ? 'destructive' : 'default'}>
            <AlertTitle>{tauriAvailable ? 'Failed to load templates' : 'Desktop app required'}</AlertTitle>
            <AlertDescription>{error}</AlertDescription>
          </Alert>
        )}

        <div className="flex gap-4 py-2">
          {/* Left column: template list */}
          <div className="flex w-64 shrink-0 flex-col gap-3">
            <Button
              variant="outline"
              size="sm"
              onClick={handleNewTemplate}
              disabled={isDisabled}
              className="gap-2"
            >
              <FilePlus2 size={14} />
              New Template
            </Button>

            <ScrollArea className="h-[420px] rounded-md border border-border bg-card">
              <div className="space-y-1 p-2">
                {loading && templates.length === 0 ? (
                  <div className="flex items-center gap-2 px-2 py-3 text-sm text-muted-foreground">
                    <Loader2 className="h-4 w-4 animate-spin" />
                    Loading templates...
                  </div>
                ) : (
                  templates.map((template) => (
                    <button
                      key={template.id}
                      type="button"
                      disabled={isDisabled}
                      onClick={() => void loadTemplateIntoEditor(template)}
                      className={`w-full rounded-md border px-3 py-2 text-left transition-colors ${
                        selectedId === template.id
                          ? 'border-border bg-muted'
                          : 'border-transparent hover:bg-muted/50'
                      } ${isDisabled ? 'cursor-not-allowed opacity-60' : ''}`}
                    >
                      <div className="flex items-center justify-between gap-2">
                        <span className="truncate text-sm font-medium text-foreground">
                          {template.name}
                        </span>
                        <span className="shrink-0 rounded bg-muted px-1.5 py-0.5 text-[10px] uppercase tracking-wide text-muted-foreground">
                          {template.is_custom ? 'Custom' : 'Built-in'}
                        </span>
                      </div>
                      <p className="truncate text-xs text-muted-foreground">
                        {template.description}
                      </p>
                    </button>
                  ))
                )}
              </div>
            </ScrollArea>
          </div>

          {/* Right column: editor */}
          <div className="flex min-w-0 flex-1 flex-col">
            <div className="flex items-center justify-between gap-2">
              <h3 className="truncate text-sm font-medium text-foreground">
                {selectedTemplate
                  ? selectedTemplate.name
                  : 'New Template'}
              </h3>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => setShowRawJson((value) => !value)}
                disabled={isDisabled}
                className="gap-1.5 text-muted-foreground"
              >
                <Braces size={14} />
                Raw JSON
              </Button>
            </div>

            {showRawJson ? (
              <ScrollArea className="mt-2 h-[380px] rounded-md border border-border bg-muted/50">
                <pre className="whitespace-pre-wrap break-words p-3 text-xs leading-relaxed text-foreground">
                  {rawJsonPreview}
                </pre>
              </ScrollArea>
            ) : (
              <ScrollArea className="mt-2 h-[380px]">
                <div className="space-y-4 pr-4">
                  <div className="space-y-2">
                    <Label htmlFor="template-name">Name</Label>
                    <Input
                      id="template-name"
                      value={name}
                      onChange={(event) => setName(event.target.value)}
                      placeholder="e.g. Engineering Sync"
                      disabled={isDisabled}
                    />
                  </div>

                  <div className="space-y-2">
                    <Label htmlFor="template-description">Description</Label>
                    <Input
                      id="template-description"
                      value={description}
                      onChange={(event) => setDescription(event.target.value)}
                      placeholder="What is this template for?"
                      disabled={isDisabled}
                    />
                  </div>

                  <Separator />

                  <div className="space-y-3">
                    <div className="flex items-center justify-between">
                      <span className="text-sm font-medium text-foreground">Sections</span>
                      <Button
                        variant="outline"
                        size="sm"
                        onClick={addSection}
                        disabled={isDisabled}
                        className="gap-1.5"
                      >
                        <Plus size={14} />
                        Add Section
                      </Button>
                    </div>

                    {sections.length === 0 && (
                      <p className="text-sm text-muted-foreground">
                        No sections yet. Add at least one section.
                      </p>
                    )}

                    {sections.map((section, index) => (
                      <div
                        key={section.id}
                        className="relative space-y-3 rounded-lg border border-border bg-card p-3"
                      >
                        <button
                          type="button"
                          onClick={() => removeSection(section.id)}
                          disabled={isDisabled}
                          aria-label={`Remove section ${index + 1}`}
                          title="Remove section"
                          className={`absolute right-2 top-2 rounded-sm p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground ${
                            isDisabled ? 'cursor-not-allowed opacity-50' : ''
                          }`}
                        >
                          <X size={14} />
                        </button>

                        <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                          <div className="space-y-2">
                            <Label>Section Title</Label>
                            <Input
                              value={section.title}
                              onChange={(event) =>
                                updateSection(section.id, { title: event.target.value })
                              }
                              placeholder={`Section ${index + 1} title`}
                              disabled={isDisabled}
                            />
                          </div>
                          <div className="space-y-2">
                            <Label>Format</Label>
                            <Select
                              value={section.format}
                              onValueChange={(value) =>
                                updateSection(section.id, { format: value as SectionFormat })
                              }
                              disabled={isDisabled}
                            >
                              <SelectTrigger>
                                <SelectValue />
                              </SelectTrigger>
                              <SelectContent>
                                <SelectItem value="paragraph">Paragraph</SelectItem>
                                <SelectItem value="list">List</SelectItem>
                                <SelectItem value="string">String</SelectItem>
                              </SelectContent>
                            </Select>
                          </div>
                        </div>

                        <div className="space-y-2">
                          <Label>Instruction</Label>
                          <Textarea
                            rows={2}
                            value={section.instruction}
                            onChange={(event) =>
                              updateSection(section.id, { instruction: event.target.value })
                            }
                            placeholder="Tell the AI what to extract or include in this section"
                            disabled={isDisabled}
                          />
                        </div>

                        {section.format === 'list' && (
                          <div className="space-y-2">
                            <Label>Item format (optional)</Label>
                            <Textarea
                              rows={2}
                              value={section.item_format}
                              onChange={(event) =>
                                updateSection(section.id, { item_format: event.target.value })
                              }
                              placeholder="e.g. - [ ] or a markdown table header"
                              disabled={isDisabled}
                            />
                          </div>
                        )}
                      </div>
                    ))}
                  </div>

                  {validationError && (
                    <p className="text-sm font-medium text-destructive">{validationError}</p>
                  )}
                </div>
              </ScrollArea>
            )}

            {confirmDelete && isCustomSelected && selectedTemplate && (
              <Alert variant="destructive" className="mt-3">
                <AlertTitle>Delete "{selectedTemplate.name}"?</AlertTitle>
                <AlertDescription>
                  This will permanently remove the template.
                  <div className="mt-3 flex gap-2">
                    <Button
                      size="sm"
                      variant="destructive"
                      onClick={handleDelete}
                      disabled={deleting}
                      className="gap-1.5"
                    >
                      {deleting ? (
                        <Loader2 className="h-4 w-4 animate-spin" />
                      ) : (
                        <Trash2 className="h-4 w-4" />
                      )}
                      Confirm Delete
                    </Button>
                    <Button
                      size="sm"
                      variant="outline"
                      onClick={() => setConfirmDelete(false)}
                      disabled={deleting}
                    >
                      Cancel
                    </Button>
                  </div>
                </AlertDescription>
              </Alert>
            )}

            <div className="mt-3 flex items-center justify-between gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => setConfirmDelete(true)}
                disabled={isDisabled || !isCustomSelected || deleting}
                className="gap-1.5 text-destructive"
              >
                <Trash2 size={14} />
                Delete
              </Button>
              <Button onClick={handleSave} disabled={isDisabled || saving} className="gap-2">
                {saving ? (
                  <Loader2 className="h-4 w-4 animate-spin" />
                ) : (
                  <Save className="h-4 w-4" />
                )}
                Save Template
              </Button>
            </div>
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
