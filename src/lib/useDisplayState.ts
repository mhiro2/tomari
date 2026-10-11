import { listen } from '@tauri-apps/api/event';
import { useCallback, useEffect, useRef, useState } from 'react';

import * as displays from './displays';
import type { DisplaySnapshot } from './displays';
import type { MessageKey } from './i18n';

export function useDisplayState() {
  const [snapshot, setSnapshot] = useState<DisplaySnapshot | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [error, setError] = useState<MessageKey | null>(null);
  const [loadError, setLoadError] = useState(false);
  const [busy, setBusy] = useState(false);
  const [loading, setLoading] = useState(true);
  const generation = useRef(0);
  const mounted = useRef(false);
  const mutating = useRef(false);
  const reading = useRef(false);
  const readQueued = useRef(false);

  const refresh = useCallback(async function refreshSnapshot() {
    if (mutating.current || reading.current) {
      readQueued.current = true;
      return;
    }
    reading.current = true;
    const request = ++generation.current;
    try {
      const next = await displays.getDisplays();
      if (mounted.current && request === generation.current) {
        setSnapshot(next);
        setLoadError(false);
      }
    } catch {
      if (mounted.current && request === generation.current) setLoadError(true);
    } finally {
      if (mounted.current && request === generation.current) setLoading(false);
      reading.current = false;
      if (readQueued.current && mounted.current) {
        readQueued.current = false;
        void refreshSnapshot();
      }
    }
  }, []);

  useEffect(() => {
    mounted.current = true;
    // Subscribe before the initial pull so connecting a display during load
    // cannot leave the view on an obsolete inventory.
    const unlisten = listen('tomari:displays-changed', () => {
      void refresh();
    });
    void unlisten.then(refresh).catch(refresh);
    const onFocus = () => {
      void refresh();
    };
    window.addEventListener('focus', onFocus);
    return () => {
      mounted.current = false;
      generation.current += 1;
      window.removeEventListener('focus', onFocus);
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, [refresh]);

  const pendingToken = snapshot?.pending?.token;
  const restoreFailed = snapshot?.notice === 'restoreFailed';
  useEffect(() => {
    if (pendingToken === undefined || restoreFailed) return;
    const timer = window.setInterval(() => {
      void refresh();
    }, 1000);
    return () => window.clearInterval(timer);
  }, [pendingToken, refresh, restoreFailed]);

  const run = useCallback(
    async (operation: () => Promise<DisplaySnapshot>) => {
      if (mutating.current) return;
      mutating.current = true;
      generation.current += 1;
      setBusy(true);
      setError(null);
      try {
        const next = await operation();
        if (mounted.current) {
          setSnapshot(next);
          setLoadError(false);
        }
      } catch (cause) {
        if (mounted.current) setError(displays.displayErrorKey(cause));
        // A native write can succeed before its status read fails. Recover the
        // current preview without erasing the action's error or requiring a tab switch.
        try {
          const current = await displays.getDisplays();
          if (mounted.current) {
            setSnapshot(current);
            setLoadError(false);
          }
        } catch {
          // The explicit Refresh action remains available when both reads fail.
        }
      } finally {
        mutating.current = false;
        if (mounted.current) {
          // The reset is in finally; the mount guard prevents a detached view update.
          // oxlint-disable-next-line react-doctor/no-loading-flag-reset-outside-finally
          setBusy(false);
          // An operation supersedes an initial read that may still be in flight.
          // oxlint-disable-next-line react-doctor/no-loading-flag-reset-outside-finally
          setLoading(false);
          // A screen change can arrive after the mutation's own inventory read.
          // Coalesce these notifications into one fresh read after it finishes.
          if (readQueued.current) {
            readQueued.current = false;
            void refresh();
          }
        }
      }
    },
    // Keep this explicit dependency for exhaustive-deps; refresh itself is stable.
    // oxlint-disable-next-line react/memo-dependencies
    [refresh],
  );

  return { snapshot, selectedId, setSelectedId, error, loadError, busy, loading, refresh, run };
}
