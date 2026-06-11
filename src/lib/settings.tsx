// Shared app-settings state. One source of truth for the whole panel so the
// master switches (which live on the Keyboard/Windows tabs) and the global
// preferences (Settings tab) read and write the same record.
//
// Writes are optimistic and serialized: the UI updates immediately, but only
// one save runs at a time and it always persists the *latest* settings. That
// makes the persistence order match the order edits were made — concurrent
// edits (a toggle here, a slider drag there, across tabs) can't race a stale
// snapshot onto disk — and slider drags coalesce into a single debounced write.
// The save error is held here so it survives a tab switch (each view unmounts
// when you leave it).

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from 'react';

import * as api from './api';
import type { AppSettings } from './types';

type SettingsContextValue = {
  settings: AppSettings | null;
  // Raw rejection from the last failed save (format with `formatCmdError` at
  // display time so this stays independent of the i18n provider).
  saveError: unknown;
  update: (patch: Partial<AppSettings>) => void;
  updateSlider: (patch: Partial<AppSettings>) => void;
  reload: () => Promise<void>;
};

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('useSettings must be used within a SettingsProvider');
  return ctx;
}

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saveError, setSaveError] = useState<unknown>(null);
  // Latest settings, so a debounced or unmount save reads the current state
  // even before React commits.
  const settingsRef = useRef<AppSettings | null>(null);
  // Pending debounced slider save, so dragging coalesces into one DB write.
  const saveTimer = useRef<number | null>(null);
  // A save is in flight; `dirty` means new edits arrived while it ran, so the
  // saver should persist the latest state once more.
  const saving = useRef(false);
  const dirty = useRef(false);
  // Holds the latest `flush` so it can re-run itself without making flush its
  // own dependency.
  const flushRef = useRef<() => Promise<void>>(null);

  useEffect(() => {
    void (async () => {
      const s = await api.getSettings();
      settingsRef.current = s;
      setSettings(s);
    })();
  }, []);

  // Flush a still-pending slider save when the whole panel goes away, rather
  // than dropping the edit. If a save is already running, hand it the latest via
  // `dirty` instead of starting a competing write that could finish out of
  // order. Best-effort: no state updates after unmount.
  useEffect(
    () => () => {
      if (saveTimer.current !== null) {
        window.clearTimeout(saveTimer.current);
        if (saving.current) {
          dirty.current = true;
        } else if (settingsRef.current) {
          void api.saveSettings(settingsRef.current);
        }
      }
    },
    [],
  );

  // Set state and ref together so a debounced save reads the current settings
  // even before React commits.
  const applySettings = useCallback((next: AppSettings) => {
    settingsRef.current = next;
    setSettings(next);
  }, []);

  // Persist the latest settings, one save at a time. New edits during a save
  // set `dirty`, so the saver re-runs and the last write reflects the final
  // state. On failure, re-sync from disk so the UI shows what truly persisted.
  const flush = useCallback(async () => {
    if (saving.current) {
      dirty.current = true;
      return;
    }
    saving.current = true;
    dirty.current = false;
    const current = settingsRef.current;
    try {
      if (current) {
        await api.saveSettings(current);
        setSaveError(null);
      }
    } catch (e) {
      // The write failed; re-sync from disk to show what truly persisted —
      // unless a newer edit arrived meanwhile, which must not be clobbered (the
      // re-run below will persist it). If the disk read also fails, keep the
      // optimistic UI as the best guess.
      try {
        const fresh = await api.getSettings();
        if (!dirty.current) applySettings(fresh);
      } catch {
        /* keep the optimistic UI */
      }
      setSaveError(e);
    } finally {
      saving.current = false;
    }
    // Edits arrived mid-save → persist the latest once more.
    if (dirty.current) await flushRef.current?.();
  }, [applySettings]);

  useEffect(() => {
    flushRef.current = flush;
  }, [flush]);

  const update = useCallback(
    (patch: Partial<AppSettings>) => {
      const previous = settingsRef.current;
      if (!previous) return;
      applySettings({ ...previous, ...patch });
      void flush();
    },
    [applySettings, flush],
  );

  // Sliders fire onChange on every step. Update the UI right away but debounce
  // the save (a DB write that also re-locks the engines) so a drag coalesces
  // into one write when it settles.
  const updateSlider = useCallback(
    (patch: Partial<AppSettings>) => {
      const previous = settingsRef.current;
      if (!previous) return;
      applySettings({ ...previous, ...patch });
      if (saveTimer.current !== null) window.clearTimeout(saveTimer.current);
      saveTimer.current = window.setTimeout(() => {
        saveTimer.current = null;
        void flush();
      }, 200);
    },
    [applySettings, flush],
  );

  // After an import replaces the whole configuration, refresh what the panel
  // shows. The Keyboard and Window tabs re-fetch their lists on their own when
  // next opened (they remount on tab switch).
  const reload = useCallback(async () => {
    const s = await api.getSettings();
    applySettings(s);
  }, [applySettings]);

  const value = useMemo(
    () => ({ settings, saveError, update, updateSlider, reload }),
    [settings, saveError, update, updateSlider, reload],
  );

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}
