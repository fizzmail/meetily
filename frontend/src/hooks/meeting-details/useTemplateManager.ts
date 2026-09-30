import { useState, useEffect, useCallback } from 'react';
import { invoke as invokeTauri } from '@tauri-apps/api/core';

export interface TemplateEntry {
  id: string;
  name: string;
  description: string;
  is_custom: boolean;
}

export const BROWSER_PREVIEW_ERROR =
  'Template management requires the desktop app (Tauri). In the browser preview, this panel is a layout preview only.';

// The two known built-in templates, used so the layout is visible in the
// plain-browser preview where the Tauri backend is not available.
export const FALLBACK_BUILTIN_TEMPLATES: TemplateEntry[] = [
  {
    id: 'standard_meeting',
    name: 'Standard Meeting Notes',
    description: 'A standard template for general meetings, focusing on key outcomes and actions.',
    is_custom: false,
  },
  {
    id: 'daily_standup',
    name: 'Daily Standup',
    description: 'Time-boxed daily updates for engineering/product teams.',
    is_custom: false,
  },
];

/**
 * True when running inside the Tauri desktop app (i.e. the native backend
 * commands are available). False in the plain-browser preview.
 */
export function isTauriAvailable(): boolean {
  return typeof window !== 'undefined' && !!window.__TAURI_INTERNALS__;
}

export function useTemplateManager() {
  const [templates, setTemplates] = useState<TemplateEntry[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!isTauriAvailable()) {
      // Browser preview: no Tauri backend. Never call invoke.
      setTemplates(FALLBACK_BUILTIN_TEMPLATES);
      setError(BROWSER_PREVIEW_ERROR);
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const list = await invokeTauri('api_list_templates_detailed') as TemplateEntry[];
      console.log('Available templates (detailed):', list);
      setTemplates(list);
    } catch (err) {
      console.error('Failed to fetch templates:', err);
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  // Fetch available templates on mount
  useEffect(() => {
    refresh();
  }, [refresh]);

  return {
    templates,
    refresh,
    loading,
    error,
  };
}
