// Shared app-settings state. One source of truth for the whole panel so the
// master switches (which live on the Keyboard/Windows tabs) and the global
// preferences (General tab) read and write the same record.
//
// Writes are optimistic and serialized: the UI updates immediately, but only
// one save runs at a time and it always persists the *latest* settings. That
// makes the persistence order match the order edits were made — concurrent
// edits (a toggle here, another there, across tabs) can't race a stale snapshot
// onto disk. The save error is held here so it survives a tab switch (each view
// unmounts when you leave it).

import { listen } from '@tauri-apps/api/event';
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
  // Codes for side effects that saved but could not be applied (see
  // `SaveSettingsOutcome`). Empty after a clean save.
  applyWarnings: string[];
  update: (patch: Partial<AppSettings>) => void;
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
  const [applyWarnings, setApplyWarnings] = useState<string[]>([]);
  // Latest settings, so an in-flight save reads the current state even before
  // React commits.
  const settingsRef = useRef<AppSettings | null>(null);
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

  // Set state and ref together so an in-flight save reads the current settings
  // even before React commits.
  const applySettings = useCallback((next: AppSettings) => {
    settingsRef.current = next;
    setSettings(next);
  }, []);

  // Adopt settings the backend broadcasts (e.g. a save applied out of band),
  // so this provider stays in step with changes it did not originate. Skip
  // while a local save is pending so an in-progress edit isn't clobbered; its
  // own flush will re-broadcast the merged result.
  useEffect(() => {
    const unlisten = listen<AppSettings>('tomari:settings-changed', (e) => {
      if (saving.current || dirty.current) return;
      applySettings(e.payload);
    });
    return () => void unlisten.then((fn) => fn());
  }, [applySettings]);

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
        const outcome = await api.saveSettings(current);
        setSaveError(null);
        // The settings persisted; surface any side effect that didn't apply.
        setApplyWarnings(outcome.applyWarnings);
      }
    } catch (e) {
      // Leave `applyWarnings` as-is: a failed save reconciled no side effect, so
      // the warnings from the last successful save still reflect the live
      // mismatch and must not be cleared here.
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

  const value = useMemo(
    () => ({ settings, saveError, applyWarnings, update }),
    [settings, saveError, applyWarnings, update],
  );

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}
