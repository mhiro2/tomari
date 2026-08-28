// Shared app-settings state. One source of truth for the whole panel so the
// master switches on the tool screens and the global preferences on General
// read and write the same record.
//
// Writes are optimistic and serialized: the UI updates immediately, but only
// one save runs at a time and it always persists the *latest* settings. That
// makes the persistence order match the order edits were made — concurrent
// edits (a toggle here, another there, across screens) can't race a stale snapshot
// onto disk. The save error is held here so it survives navigation (each view
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
  // Raw rejection from the last failed initial load (format with
  // `formatCmdError` at display time so this stays independent of the i18n
  // provider). `settings` stays null while this is set, so consumers can show
  // an error + retry in place of the perpetual loading state.
  loadError: unknown;
  // Re-runs the initial load after `loadError` was set.
  retryLoad: () => void;
  // Raw rejection from the last failed save (format with `formatCmdError` at
  // display time so this stays independent of the i18n provider).
  saveError: unknown;
  // Codes for side effects that saved but could not be applied (see
  // `SaveSettingsOutcome`). Empty after a clean save.
  applyWarnings: string[];
  update: (patch: Partial<AppSettings>) => void;
  // Fold the outcome of a mutation made outside `update` — a modifier-rule
  // save, say — into `applyWarnings`. `probed` lists the codes that mutation
  // is able to report on; those are replaced by what it reported, every other
  // code keeps the verdict it already had. One shared state, so a warning
  // survives leaving the page and clears on the next clean apply wherever that
  // happens.
  reportApplyOutcome: (outcome: { applyWarnings: string[] }, probed: readonly string[]) => void;
};

const SettingsContext = createContext<SettingsContextValue | null>(null);

export function useSettings(): SettingsContextValue {
  const ctx = useContext(SettingsContext);
  if (!ctx) throw new Error('useSettings must be used within a SettingsProvider');
  return ctx;
}

export function SettingsProvider({ children }: { children: ReactNode }) {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [loadError, setLoadError] = useState<unknown>(null);
  const [loadAttempt, setLoadAttempt] = useState(0);
  const [saveError, setSaveError] = useState<unknown>(null);
  const [applyWarnings, setApplyWarnings] = useState<string[]>([]);
  // Bumped whenever a live-state read starts and whenever a save's outcome
  // sets `applyWarnings`, so only the newest read may apply its result: an
  // older one resolving later cannot overwrite a fresher read or the
  // post-save list with what it saw before.
  const warningsGeneration = useRef(0);
  // Latest settings, so an in-flight save reads the current state even before
  // React commits.
  const settingsRef = useRef<AppSettings | null>(null);
  // A save is in flight; `dirty` means new edits arrived while it ran, so the
  // saver should persist the latest state once more.
  const saving = useRef(false);
  const dirty = useRef(false);
  // Set once any settings have been adopted (from the initial load or a
  // broadcast event), so a slow initial load that resolves after a broadcast
  // does not clobber the newer snapshot with a stale one.
  const settled = useRef(false);
  // Holds the latest `flush` so it can re-run itself without making flush its
  // own dependency.
  const flushRef = useRef<() => Promise<void>>(null);

  // Read the warnings the live state warrants right now. Best effort: a
  // failure leaves the current list alone, and the next save's outcome
  // replaces it either way. A result is dropped when a save landed or a newer
  // read started meanwhile (either is fresher) or the caller has unmounted.
  const refreshApplyWarnings = useCallback(async (isCancelled: () => boolean) => {
    warningsGeneration.current += 1;
    const generation = warningsGeneration.current;
    try {
      const live = await api.getApplyWarnings();
      if (isCancelled() || warningsGeneration.current !== generation) return;
      if (!live || !Array.isArray(live.warnings)) return;
      // Codes the live read has no probe for keep the last save's verdict —
      // their absence from `warnings` means "not checked", not "healed".
      const unprobed = new Set(live.unprobed ?? []);
      setApplyWarnings((prev) => [...live.warnings, ...prev.filter((code) => unprobed.has(code))]);
    } catch {
      // ignore — surfaced again by the next save
    }
  }, []);

  // The window is hidden rather than destroyed when the panel closes, so a
  // mismatch that arose while it was hidden (a `hidutil` timeout during the
  // wake reset, say) has to be re-read each time it is shown again.
  useEffect(() => {
    let cancelled = false;
    const unlisten = listen('tomari:panel-shown', () => {
      void refreshApplyWarnings(() => cancelled);
    });
    return () => {
      cancelled = true;
      void unlisten.then((fn) => fn());
    };
  }, [refreshApplyWarnings]);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        const s = await api.getSettings();
        if (cancelled || settled.current) return;
        settled.current = true;
        settingsRef.current = s;
        setSettings(s);
        setLoadError(null);
        // Seed the warnings from the live state so one that outlived the last
        // save (a restore that failed on quit, retried at this launch) shows
        // without waiting for a save.
        void refreshApplyWarnings(() => cancelled);
      } catch (e) {
        if (cancelled || settled.current) return;
        setLoadError(e);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadAttempt, refreshApplyWarnings]);

  const retryLoad = useCallback(() => {
    setLoadError(null);
    setLoadAttempt((n) => n + 1);
  }, []);

  // Set state and ref together so an in-flight save reads the current settings
  // even before React commits. Also marks the provider as settled (so a slow
  // initial load that resolves afterward is discarded instead of clobbering
  // this newer snapshot) and clears any stale `loadError`, keeping the
  // invariant that `settings` and `loadError` are never both set.
  const applySettings = useCallback((next: AppSettings) => {
    settled.current = true;
    settingsRef.current = next;
    setSettings(next);
    setLoadError(null);
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
        warningsGeneration.current += 1;
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

  const reportApplyOutcome = useCallback(
    (outcome: { applyWarnings: string[] }, probed: readonly string[]) => {
      // Fresher than any live read still in flight.
      warningsGeneration.current += 1;
      const replaced = new Set(probed);
      setApplyWarnings((prev) => [
        ...prev.filter((code) => !replaced.has(code)),
        ...outcome.applyWarnings,
      ]);
    },
    [],
  );

  const value = useMemo(
    () => ({
      settings,
      loadError,
      retryLoad,
      saveError,
      applyWarnings,
      update,
      reportApplyOutcome,
    }),
    [settings, loadError, retryLoad, saveError, applyWarnings, update, reportApplyOutcome],
  );

  return <SettingsContext.Provider value={value}>{children}</SettingsContext.Provider>;
}
