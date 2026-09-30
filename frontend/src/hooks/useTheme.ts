import { useState, useEffect, useCallback } from 'react';
import { getCurrentWindow } from '@tauri-apps/api/window';

export type Theme = 'light' | 'dark';

const THEME_STORAGE_KEY = 'theme';

/**
 * Apply the theme to the native window title bar (OS-drawn caption bar).
 * No-op outside a Tauri webview (e.g. the plain-browser preview).
 */
async function applyNativeTheme(theme: Theme): Promise<void> {
  if (typeof window === 'undefined' || !window.__TAURI_INTERNALS__) return;
  try {
    await getCurrentWindow().setTheme(theme);
  } catch (error) {
    console.error('[useTheme] Failed to set native window theme:', error);
  }
}

/**
 * Read the persisted theme from localStorage
 * @returns The stored theme, or "light" if not set / invalid
 */
export function getStoredTheme(): Theme {
  if (typeof window === 'undefined') return 'light';
  try {
    const stored = window.localStorage.getItem(THEME_STORAGE_KEY);
    return stored === 'dark' ? 'dark' : 'light';
  } catch {
    return 'light';
  }
}

/**
 * Apply the stored theme to the document root.
 * Safe to call before React mounts / on app startup.
 */
export function applyStoredTheme(): void {
  if (typeof document === 'undefined') return;
  const theme = getStoredTheme();
  document.documentElement.classList.toggle('dark', theme === 'dark');
  void applyNativeTheme(theme);
}

/**
 * Hook to manage the app theme (light/dark).
 * Persists to localStorage and toggles the `dark` class on <html>.
 * @returns The current theme and a setter that persists the choice
 */
export function useTheme(): { theme: Theme; setTheme: (theme: Theme) => void } {
  const [theme, setThemeState] = useState<Theme>(() => getStoredTheme());

  useEffect(() => {
    document.documentElement.classList.toggle('dark', theme === 'dark');
    void applyNativeTheme(theme);
  }, [theme]);

  const setTheme = useCallback((nextTheme: Theme) => {
    try {
      window.localStorage.setItem(THEME_STORAGE_KEY, nextTheme);
    } catch (error) {
      console.error('[useTheme] Failed to persist theme:', error);
    }
    setThemeState(nextTheme);
  }, []);

  return { theme, setTheme };
}
